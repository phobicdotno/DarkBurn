//! MCC100 register map: blocks, word offsets, bit tables, and the read/write deny rules.
//!
//! Everything here comes from the owner's static analysis (`11-static-findings.md` sections 3 and 4,
//! `11-static/A2-status-words.md`) and is pure data; there is no I/O. Facts the analysis marks as
//! inferred are listed in [`UNVERIFIED_NOTES`] and in `docs/hardware-checks-mcc.md`.
//!
//! Word `i` of block `B` is register `B + i`; all words are `u32`.

use std::fmt;

/// Command register 101 (0x65). Only the vectors in [`crate::command`] can be written to it.
pub const REG_COMMAND: u32 = 0x65;
/// FIFO data register 102 (0x66).
pub const REG_FIFO_DATA: u32 = 0x66;
/// FIFO run control 103 (0x67): 1 clear, 2 start, 3 stop.
pub const REG_FIFO_CONTROL: u32 = 0x67;

/// First licence register. Never readable or writable through this crate.
pub const LICENCE_FIRST: u32 = 59500;
/// Last licence / card clock register (inclusive).
pub const LICENCE_LAST: u32 = 59599;
/// Hardware-parameter blocks start at `59600 + 0xD0 * i`.
pub const HARD_PARAM_BASE: u32 = 59600;
/// Stride of the hardware-parameter blocks.
pub const HARD_PARAM_STRIDE: u32 = 0xD0;
/// Words per hardware-parameter block.
pub const HARD_PARAM_BLOCK_WORDS: u32 = 52;

/// Number of axis slots: 0 X, 1 Y, 2 Y2, 3 height axis, 4 W / lifting table.
pub const AXIS_SLOTS: usize = 5;
/// Words per slot in block 2000.
pub const AXIS_RO_WORDS: usize = 10;
/// Words per slot of block 50200 on the wire.
pub const AXIS_RW_WORDS_ON_WIRE: usize = 20;

/// Register 1016 reads 60000 when the FIFO is empty.
pub const FIFO_MARGIN_EMPTY: u32 = 60000;
/// The vendor sends a FIFO frame only if `frame_bytes + 2000 <= reg 1016`.
pub const FIFO_SEND_HEADROOM_BYTES: u32 = 2000;
/// Register 1031 equal to 1 means the card needs a restart; refuse motion.
pub const PARAM_STATUS_RESTART_REQUIRED: u32 = 1;

/// How a block may be touched through this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reads allowed, writes impossible.
    Read,
    /// One-off diagnostic read only (block 5000, never read by the vendor).
    Diagnostic,
    /// Write-only command / FIFO register, writes only through typed constructors.
    Command,
}

/// A register block as the vendor addresses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// Short name.
    pub name: &'static str,
    /// First register.
    pub start: u32,
    /// Word count.
    pub count: u32,
    /// Access class.
    pub access: Access,
    /// What it holds.
    pub doc: &'static str,
}

impl Block {
    /// One past the last register.
    pub const fn end(&self) -> u32 {
        self.start + self.count
    }

    /// True if `[addr, addr + count)` lies inside the block.
    pub const fn contains(&self, addr: u32, count: u32) -> bool {
        match addr.checked_add(count) {
            Some(end) => self.start <= addr && end <= self.end(),
            None => false,
        }
    }
}

/// The blocks the vendor software reads or writes.
pub const BLOCKS: &[Block] = &[
    Block { name: "status", start: 1000, count: 36, access: Access::Read, doc: "read-only status; words 0..31 named in `Status`" },
    Block { name: "version_time", start: 1050, count: 3, access: Access::Read, doc: "version / time triple, seen in logs only" },
    Block { name: "axis_ro", start: 2000, count: 50, access: Access::Read, doc: "axis read-only, 5 slots x 10 words" },
    Block { name: "rw_5000", start: 5000, count: 9, access: Access::Diagnostic, doc: "e-stop port / safety decel; vendor never reads it" },
    Block { name: "zf_status", start: 10000, count: 18, access: Access::Read, doc: "on-board height follower status, only if ZFType != 0" },
    Block { name: "zf_prop", start: 11000, count: 39, access: Access::Read, doc: "follower properties; writes denied" },
    Block { name: "system_rw", start: 50000, count: 26, access: Access::Read, doc: "system block; card is authoritative for words 5, 6, 17" },
    Block { name: "axis_rw", start: 50200, count: 100, access: Access::Read, doc: "axis read/write, 5 slots x 20 words (vendor keeps 14)" },
    Block { name: "fast", start: 60001, count: 120, access: Access::Read, doc: "2000 block plus compacted 50200 block in one read" },
    Block { name: "command", start: REG_COMMAND, count: 1, access: Access::Command, doc: "0x65 command vectors" },
    Block { name: "fifo_data", start: REG_FIFO_DATA, count: 1, access: Access::Command, doc: "0x66 FIFO frames" },
    Block { name: "fifo_control", start: REG_FIFO_CONTROL, count: 1, access: Access::Command, doc: "0x67: 1 clear, 2 start, 3 stop" },
];

/// Look a block up by name.
pub fn block(name: &str) -> Option<&'static Block> {
    BLOCKS.iter().find(|b| b.name == name)
}

/// Word index inside the status block at 1000.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    /// Program id.
    ProgramId = 0,
    /// Program (firmware) version; compared against `MinHardwareVer`.
    ProgramVersion = 1,
    /// Date.
    Date = 2,
    /// Time.
    Time = 3,
    /// Digital inputs 1..12 (bits 0..11).
    Di = 4,
    /// Digital outputs 1..10 (bits 0..9).
    Do = 5,
    /// Alarm word 1, see [`alarm1`].
    Alarm1 = 6,
    /// Alarm word 2, see [`alarm2`].
    Alarm2 = 7,
    /// Run status (no vendor consumer, meaning unknown).
    RunStatus = 8,
    /// Analogue sample.
    AdSample = 9,
    /// DA 1.
    Da1 = 10,
    /// DA 2.
    Da2 = 11,
    /// DA 3.
    Da3 = 12,
    /// PWM frequency.
    PwmFreq = 13,
    /// PWM duty.
    PwmDuty = 14,
    /// Last accepted FIFO frame id.
    FifoFrameId = 15,
    /// FIFO space margin (60000 when empty; unit unconfirmed).
    FifoSpaceMargin = 16,
    /// FIFO interpolation config.
    FifoInterpConfig = 17,
    /// Slave device info.
    SlaveDeviceInfo = 18,
    /// Processing status; low byte 1 means the FIFO program runs.
    ProcessingStatus = 19,
    /// Processing position.
    ProcessingPosition = 20,
    /// Extension inputs 13..28.
    ExtDi = 21,
    /// Extension outputs 11..26.
    ExtDo = 22,
    /// Contour status.
    ContourStatus = 23,
    /// Contour index.
    ContourIndex = 24,
    /// Power-on time.
    PowerOnTime = 25,
    /// Communication time.
    CommTime = 26,
    /// Laser-on time.
    LaserOnTime = 27,
    /// Dual drive deviation.
    DualDriveDeviation = 28,
    /// Sampling encoder config.
    SamplingEncoderCfg = 29,
    /// Sampling encoder length.
    SamplingEncoderLen = 30,
    /// Parameter status; 1 means restart required.
    ParamStatus = 31,
}

impl Status {
    /// Absolute register address (`1000 + index`).
    pub const fn addr(self) -> u32 {
        1000 + self as u32
    }
}

/// Typed view of the status block at 1000. Only words with evidence are given a meaning.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBlock {
    /// All words as read (at least 32).
    pub words: Vec<u32>,
}

impl StatusBlock {
    /// Wrap a block read. Needs at least the 32 named words.
    pub fn from_words(words: &[u32]) -> Option<StatusBlock> {
        if words.len() < 32 {
            return None;
        }
        Some(StatusBlock { words: words.to_vec() })
    }

    /// One named word.
    pub fn word(&self, w: Status) -> u32 {
        self.words[w as usize]
    }

    /// Alarm word 1.
    pub fn alarm1(&self) -> u32 {
        self.word(Status::Alarm1)
    }

    /// Alarm word 2.
    pub fn alarm2(&self) -> u32 {
        self.word(Status::Alarm2)
    }

    /// Last accepted FIFO frame id (register 1015).
    pub fn fifo_frame_id(&self) -> u32 {
        self.word(Status::FifoFrameId)
    }

    /// FIFO space margin (register 1016).
    pub fn fifo_space_margin(&self) -> u32 {
        self.word(Status::FifoSpaceMargin)
    }

    /// True when register 1019 low byte is 1: the FIFO program runs.
    pub fn fifo_running(&self) -> bool {
        self.word(Status::ProcessingStatus) & 0xFF == 1
    }

    /// True when the card asks for a restart (register 1031 is 1).
    pub fn restart_required(&self) -> bool {
        self.word(Status::ParamStatus) == PARAM_STATUS_RESTART_REQUIRED
    }

    /// Digital output `port` (1..=10 in word 5, 11..=26 in word 22).
    pub fn do_is_on(&self, port: u32) -> bool {
        match port {
            1..=10 => (self.word(Status::Do) >> (port - 1)) & 1 == 1,
            11..=26 => (self.word(Status::ExtDo) >> (port - 11)) & 1 == 1,
            _ => false,
        }
    }

    /// Raw level of digital input `port` (1..=12 in word 4, 13..=28 in word 21). Whether the card
    /// already applies the input-type inversion is unconfirmed.
    pub fn di_level(&self, port: u32) -> bool {
        match port {
            1..=12 => (self.word(Status::Di) >> (port - 1)) & 1 == 1,
            13..=28 => (self.word(Status::ExtDi) >> (port - 13)) & 1 == 1,
            _ => false,
        }
    }

    /// Digital input `port` with the PC-side input type applied: a normally-closed input is
    /// active when its level is low. Port 0 (not assigned) is never active. Whether the card
    /// already inverts the level itself is unconfirmed.
    pub fn di_active(&self, port: u32, normally_closed: bool) -> bool {
        if !(1..=28).contains(&port) {
            return false;
        }
        self.di_level(port) != normally_closed
    }

    /// Id the next FIFO frame must carry: register 1015 plus one.
    pub fn next_frame_id(&self) -> u32 {
        self.fifo_frame_id().wrapping_add(1)
    }

    /// Program id and version (words 0 and 1).
    pub fn version(&self) -> VersionInfo {
        VersionInfo { program_id: self.word(Status::ProgramId), program_version: self.word(Status::ProgramVersion) }
    }
}

/// Lowest card program version the owner's vendor release 0.0.1.44 accepts.
pub const MIN_PROGRAM_VERSION: u32 = 20152;
/// Lowest card program version vendor release 0.0.1.51 SP1 accepts.
pub const MIN_PROGRAM_VERSION_SP1: u32 = 20177;

/// Answer to the first read of a connect: `READ 1000/2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    /// Register 1000.
    pub program_id: u32,
    /// Register 1001; compared against the minimum the PC software accepts.
    pub program_version: u32,
}

impl VersionInfo {
    /// Decode the two words of `READ 1000/2` (or the start of a longer status read).
    pub fn from_words(words: &[u32]) -> Option<VersionInfo> {
        match words {
            [id, ver, ..] => Some(VersionInfo { program_id: *id, program_version: *ver }),
            _ => None,
        }
    }

    /// True when the card's program is at least `minimum`.
    pub fn meets(&self, minimum: u32) -> bool {
        self.program_version >= minimum
    }
}

/// Text for every set bit of the two alarm words, in our own words. Bits the analysis could not
/// name are reported with their number.
pub fn alarm_labels(alarm1: u32, alarm2: u32) -> Vec<String> {
    let mut out = Vec::new();
    for bit in 0..32u32 {
        if (alarm1 >> bit) & 1 == 0 {
            continue;
        }
        out.push(match bit {
            0 => "axis X fault".to_string(),
            1 => "axis Y fault".to_string(),
            2 => "axis Y2 fault".to_string(),
            3 => "height axis fault".to_string(),
            4 => "axis W fault".to_string(),
            5..=23 => format!("axis fault (alarm 1 bit {bit})"),
            24 => "follower axis (unconfirmed)".to_string(),
            25 => "bus fault".to_string(),
            26 => "output fault".to_string(),
            30 => "emergency stop".to_string(),
            _ => format!("alarm 1 bit {bit} (no known meaning)"),
        });
    }
    for bit in 0..32u32 {
        if (alarm2 >> bit) & 1 == 0 {
            continue;
        }
        out.push(match bit {
            0 => "illegal command".to_string(),
            1 => "interpolation length".to_string(),
            2 => "axis command".to_string(),
            3 => "follower command".to_string(),
            4 => "sequence (PLC) command".to_string(),
            5 => "motion queue ran empty".to_string(),
            _ => format!("alarm 2 bit {bit} (no known meaning)"),
        });
    }
    out
}

/// Alarm word 1 (register 1006) bits, 1 = active.
pub mod alarm1 {
    /// Axis X fault.
    pub const AXIS_X: u32 = 1 << 0;
    /// Axis Y fault.
    pub const AXIS_Y: u32 = 1 << 1;
    /// Axis Y2 fault.
    pub const AXIS_Y2: u32 = 1 << 2;
    /// Height axis fault.
    pub const AXIS_HEIGHT: u32 = 1 << 3;
    /// W axis fault.
    pub const AXIS_W: u32 = 1 << 4;
    /// Bits 5..23, generic axis fault without a status word.
    pub const AXIS_GENERIC: u32 = 0x00FF_FFE0;
    /// On-board follower axis (inferred, low-medium confidence).
    pub const FOLLOWER_AXIS: u32 = 1 << 24;
    /// Bus fault.
    pub const BUS_FAULT: u32 = 1 << 25;
    /// Output fault.
    pub const OUTPUT_FAULT: u32 = 1 << 26;
    /// E-stop.
    pub const ESTOP: u32 = 1 << 30;
    /// Bits with empty vendor strings; treat any as an unknown alarm.
    pub const RESERVED_MASK: u32 = (1 << 27) | (1 << 28) | (1 << 29) | (1 << 31);
}

/// Alarm word 2 (register 1007) bits 0..5.
pub mod alarm2 {
    /// Illegal command.
    pub const ILLEGAL_COMMAND: u32 = 1 << 0;
    /// Interpolation data length.
    pub const INTERP_LENGTH: u32 = 1 << 1;
    /// Axis command.
    pub const AXIS_COMMAND: u32 = 1 << 2;
    /// FTC command.
    pub const FTC_COMMAND: u32 = 1 << 3;
    /// PLC command.
    pub const PLC_COMMAND: u32 = 1 << 4;
    /// FIFO starvation.
    pub const FIFO_STARVATION: u32 = 1 << 5;
}

/// A raised alarm: the vendor numeric code and a short label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlarmCode {
    /// Vendor numeric code (8000 + bit, 8100 + ..., 9000 + bit).
    pub code: u32,
    /// Short label.
    pub label: String,
}

/// Codes the vendor poll routine would raise for the two alarm words. The vendor treats
/// `alarm1 == 0x0100_0000` alone as "no alarm" under unnamed gates; this function reports it.
pub fn alarm_codes(alarm1: u32, alarm2: u32, axis_ro: Option<&[u32]>) -> Vec<AlarmCode> {
    let mut out = Vec::new();
    for bit in 0..32u32 {
        if (alarm1 >> bit) & 1 == 0 {
            continue;
        }
        if bit <= 24 {
            let mut detail = false;
            if (bit as usize) < AXIS_SLOTS
                && let Some(ro) = axis_ro
                && let Some(&word) = ro.get(AXIS_RO_WORDS * bit as usize)
            {
                for k in 0..6u32 {
                    if (word >> k) & 1 == 1 {
                        out.push(AlarmCode { code: 8100 + bit * 32 + k, label: format!("axis {bit} status bit {k}") });
                        detail = true;
                    }
                }
            }
            if !detail {
                out.push(AlarmCode { code: 8000 + bit, label: "axis fault".into() });
            }
        } else if matches!(bit, 25 | 26 | 30) {
            out.push(AlarmCode { code: 8000 + bit, label: format!("alarm 1 bit {bit}") });
        } else {
            out.push(AlarmCode { code: 8000 + bit, label: "unknown".into() });
        }
    }
    for bit in 0..6u32 {
        if (alarm2 >> bit) & 1 == 1 {
            out.push(AlarmCode { code: 9000 + bit, label: format!("alarm 2 bit {bit}") });
        }
    }
    if alarm2 & !0x3F != 0 {
        out.push(AlarmCode { code: 9099, label: "unknown".into() });
    }
    out
}

/// Axis status word helpers (word 0 of each slot in block 2000).
pub mod axis {
    /// Homed (bit 15, inferred high confidence).
    pub const HOMED: u32 = 1 << 15;
    /// Command type: home.
    pub const CMD_HOME: u32 = 2;
    /// Command type: jog (3, 4 and 5 are jog / move variants).
    pub const CMD_JOG: u32 = 3;
    /// Hard limit, positive side (which switch is which is a bench check).
    pub const HARD_LIMIT_POS: u32 = 1 << 0;
    /// Hard limit, negative side.
    pub const HARD_LIMIT_NEG: u32 = 1 << 1;
    /// Soft limit, positive side.
    pub const SOFT_LIMIT_POS: u32 = 1 << 2;
    /// Soft limit, negative side.
    pub const SOFT_LIMIT_NEG: u32 = 1 << 3;
    /// Servo drive alarm.
    pub const SERVO_ALARM: u32 = 1 << 4;
    /// Dual-drive (gantry) deviation alarm.
    pub const DUAL_DRIVE_ALARM: u32 = 1 << 5;
    /// The six fault bits.
    pub const FAULT_MASK: u32 = 0x3F;

    /// Names of the fault bits set in an axis status word.
    pub fn fault_names(status_word: u32) -> Vec<&'static str> {
        const NAMES: [&str; 6] = ["hard limit +", "hard limit -", "soft limit +", "soft limit -", "servo alarm", "dual-drive alarm"];
        (0..6).filter(|b| (status_word >> b) & 1 == 1).map(|b| NAMES[b]).collect()
    }

    /// Byte 2 non-zero means a command is executing.
    pub const fn busy(status_word: u32) -> bool {
        (status_word >> 16) & 0xFF != 0
    }

    /// Byte 3: current command type.
    pub const fn command_type(status_word: u32) -> u32 {
        (status_word >> 24) & 0xFF
    }

    /// Register of word `word` of `slot` in block 2000.
    pub const fn ro_addr(slot: usize, word: usize) -> Option<u32> {
        if slot < super::AXIS_SLOTS && word < super::AXIS_RO_WORDS { Some(2000 + (super::AXIS_RO_WORDS * slot + word) as u32) } else { None }
    }

    /// Register of word `word` of `slot` in block 50200.
    pub const fn rw_addr(slot: usize, word: usize) -> Option<u32> {
        if slot < super::AXIS_SLOTS && word < super::AXIS_RW_WORDS_ON_WIRE {
            Some(50200 + (super::AXIS_RW_WORDS_ON_WIRE * slot + word) as u32)
        } else {
            None
        }
    }
}

/// Machine state derived from card words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineState {
    /// Ready.
    Ready,
    /// Homing.
    Origin,
    /// Jogging.
    Jog,
    /// FIFO program running.
    Process,
}

/// Derive the state like the vendor VM: process beats origin beats jog beats ready.
/// `axis_ro` is the 50-word block 2000 read. Stop and alarm states are set by the vendor UI.
pub fn derive_machine_state(status: &StatusBlock, axis_ro: &[u32]) -> MachineState {
    if status.fifo_running() {
        return MachineState::Process;
    }
    for slot in 0..AXIS_SLOTS {
        let Some(&word) = axis_ro.get(AXIS_RO_WORDS * slot) else { continue };
        if !axis::busy(word) {
            continue;
        }
        match axis::command_type(word) {
            axis::CMD_HOME => return MachineState::Origin,
            3..=5 => return MachineState::Jog,
            _ => {}
        }
    }
    MachineState::Ready
}

/// Typed view of the follower status block at 10000 (18 words). Only words 1 to 3 have evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZfStatus {
    /// All 18 words as read.
    pub words: Vec<u32>,
}

/// Follower alarm word bits (`words[1] & 0xFFFF`).
pub mod zf_alarm {
    /// Z hard limit up.
    pub const Z_HARD_UP: u32 = 1 << 0;
    /// Z hard limit down.
    pub const Z_HARD_DOWN: u32 = 1 << 1;
    /// Z soft limit up.
    pub const Z_SOFT_UP: u32 = 1 << 2;
    /// Z soft limit down.
    pub const Z_SOFT_DOWN: u32 = 1 << 3;
    /// Servo input.
    pub const SERVO_INPUT: u32 = 1 << 4;
    /// Touch plate.
    pub const TOUCH_PLATE: u32 = 1 << 5;
    /// Encoder.
    pub const ENCODER: u32 = 1 << 6;
    /// Signal too small.
    pub const SIGNAL_SMALL: u32 = 1 << 7;
    /// Follow error.
    pub const FOLLOW_ERROR: u32 = 1 << 8;
    /// Capacitance too small.
    pub const CAPACITANCE_SMALL: u32 = 1 << 9;
    /// Signal too large.
    pub const SIGNAL_LARGE: u32 = 1 << 10;
    /// FPGA not loaded.
    pub const FPGA_NOT_LOADED: u32 = 1 << 11;
    /// Axis value.
    pub const AXIS_VALUE: u32 = 1 << 12;
    /// Signal zero.
    pub const SIGNAL_ZERO: u32 = 1 << 13;
    /// FTC alarm.
    pub const FTC_ALARM: u32 = 1 << 15;

    /// Names of the bits set in a follower alarm word. Bit 14 has no known meaning.
    pub fn names(alarm: u32) -> Vec<&'static str> {
        const NAMES: [&str; 16] = [
            "Z hard limit up",
            "Z hard limit down",
            "Z soft limit up",
            "Z soft limit down",
            "servo input",
            "touched the plate",
            "encoder",
            "signal too small",
            "follow error",
            "capacitance too small",
            "signal too large",
            "follower logic not loaded",
            "axis value",
            "signal zero",
            "bit 14 (no known meaning)",
            "follower alarm",
        ];
        (0..16).filter(|b| (alarm >> b) & 1 == 1).map(|b| NAMES[b]).collect()
    }
}

/// Word index inside the follower status block at 10000, in the order of the vendor's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum ZfWord {
    /// General information.
    Info = 0,
    /// Alarm word, see [`zf_alarm`].
    Alarm = 1,
    /// Run status (high bit clear means emergency stop, low byte 4 means piercing).
    RunStatus = 2,
    /// Run command (102..=107).
    RunCommand = 3,
    /// Signal strength.
    SignalStrength = 4,
    /// Signal height.
    SignalHeight = 5,
    /// Z axis position.
    ZPosition = 6,
    /// Current speed.
    Speed = 7,
    /// Follow error.
    FollowError = 8,
    /// Follow time.
    FollowTime = 9,
    /// Sheet coordinate.
    SheetCoordinate = 10,
    /// Real-time signal strength.
    LiveSignalStrength = 11,
    /// Signal compensation.
    SignalCompensation = 12,
    /// Soft lower-limit coordinate.
    SoftLowerLimit = 13,
    /// Edge-seek sample count.
    EdgeSeekSamples = 14,
    /// Point of the largest edge-seek deviation.
    EdgeSeekMaxDeviation = 15,
    /// Position of the last edge-seek record.
    EdgeSeekLastRecord = 16,
    /// Other information.
    Other = 17,
}

/// Follower run state, derived like the vendor VM from words 2 and 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZfState {
    /// Ready.
    Ready,
    /// Following the sheet.
    Follow,
    /// Piercing.
    Drill,
    /// Jogging down.
    JogDown,
    /// Emergency stop (high bit of the status word clear).
    Estop,
    /// Calibration.
    Calibration,
    /// A state the analysis could not name.
    Other(u32),
}

impl ZfStatus {
    /// Wrap a block read of 18 words.
    pub fn from_words(words: &[u32]) -> Option<ZfStatus> {
        (words.len() >= 18).then(|| ZfStatus { words: words.to_vec() })
    }

    /// Follower alarm word.
    pub fn alarm(&self) -> u32 {
        self.words[1] & 0xFFFF
    }

    /// One word by name. Only [`ZfWord::Alarm`], [`ZfWord::RunStatus`] and [`ZfWord::RunCommand`]
    /// have evidence for their meaning; the others carry the name of their place in the vendor's
    /// table and no confirmed unit.
    pub fn word(&self, w: ZfWord) -> u32 {
        self.words[w as usize]
    }

    /// Names of the alarm bits that are set.
    pub fn alarm_names(&self) -> Vec<&'static str> {
        zf_alarm::names(self.alarm())
    }

    /// Follower state.
    pub fn state(&self) -> ZfState {
        let s = self.words[2];
        let c = self.words[3];
        if s & 0x8000_0000 == 0 {
            return ZfState::Estop;
        }
        if s & 0xFF == 4 {
            return ZfState::Drill;
        }
        match c {
            102 => ZfState::Other(6),
            103 => ZfState::JogDown,
            104 => ZfState::Follow,
            105 => ZfState::Ready,
            106 => ZfState::Other(8),
            107 => ZfState::Calibration,
            _ => ZfState::Ready,
        }
    }
}

/// Why a register access or command cannot be represented by this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Forbidden {
    /// Registers 150 / 151: follower factory reset / commit and edge-seek buffer, also the card's
    /// licence "access switch" in earlier analysis. Never touched.
    Register150Or151(u32),
    /// Licence / clock area 59500..=59599.
    LicenceArea(u32),
    /// Hardware parameters 59600 and up (read-compare only, and only through a capture-verified path).
    HardwareParameters(u32),
    /// Follower property writes (11000..).
    ZfProperty(u32),
    /// Register 100 (restart / apply, meaning unproven).
    Register100,
    /// Block 5000..=5012: e-stop port / safety decel.
    Block5000(u32),
    /// Test-data window 200 / 201.
    TestData(u32),
    /// A 0x65 vector outside the allow-list.
    Command(String),
    /// A FIFO frame that is malformed or carries an item outside the allow-list.
    FifoFrame(String),
    /// Laser-enabling data. This crate cannot encode it.
    LaserData(String),
    /// Control writes other than the values 1, 2 and 3.
    FifoControl,
    /// Address not in the write allow-list.
    NotInAllowList(u32),
    /// Read not in the read allow-list.
    ReadNotAllowed(u32, u32),
    /// Function code with no safe representation (0x26 byte block, 0x20 / 0x21 streams).
    Function(u8),
}

impl fmt::Display for Forbidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Forbidden::Register150Or151(a) => write!(f, "register {a}: follower reset/commit and edge-seek buffer"),
            Forbidden::LicenceArea(a) => write!(f, "register {a}: licence / clock area 59500-59599"),
            Forbidden::HardwareParameters(a) => write!(f, "register {a}: hardware-parameter area"),
            Forbidden::ZfProperty(a) => write!(f, "register {a}: follower property write"),
            Forbidden::Register100 => write!(f, "register 100: restart/apply, meaning unproven"),
            Forbidden::Block5000(a) => write!(f, "register {a}: e-stop port / safety decel block"),
            Forbidden::TestData(a) => write!(f, "register {a}: test-data window"),
            Forbidden::Command(s) => write!(f, "command vector refused: {s}"),
            Forbidden::FifoFrame(s) => write!(f, "FIFO frame refused: {s}"),
            Forbidden::LaserData(s) => write!(f, "laser data cannot be encoded by this crate: {s}"),
            Forbidden::FifoControl => write!(f, "register 0x67 accepts only [1], [2], [3]"),
            Forbidden::NotInAllowList(a) => write!(f, "register {a} is not in the write allow-list"),
            Forbidden::ReadNotAllowed(a, n) => write!(f, "read {a}/{n} is not in the read allow-list"),
            Forbidden::Function(func) => write!(f, "function {func:#04x} has no safe representation"),
        }
    }
}

impl std::error::Error for Forbidden {}

const READ_ALLOW: &[(u32, u32)] =
    &[(1000, 36), (1050, 3), (2000, 50), (5000, 9), (10000, 18), (10028, 2), (11000, 39), (12002, 800), (50000, 26), (50200, 100), (60001, 144)];

/// Is `READ addr/count` allowed? Mirrors the owner's read allow-list; licence and clock registers,
/// 150/151 and 200 are always refused.
pub fn check_read(addr: u32, count: u32) -> Result<(), Forbidden> {
    if count == 0 || count > 250 {
        return Err(Forbidden::ReadNotAllowed(addr, count));
    }
    let end = addr.saturating_add(count);
    if addr <= LICENCE_LAST && end > LICENCE_FIRST {
        return Err(Forbidden::LicenceArea(addr));
    }
    if addr <= 151 && end > 150 {
        return Err(Forbidden::Register150Or151(addr));
    }
    if addr <= 200 && end > 200 {
        return Err(Forbidden::TestData(addr));
    }
    if READ_ALLOW.iter().any(|&(s, n)| s <= addr && end <= s + n) {
        return Ok(());
    }
    if addr >= HARD_PARAM_BASE {
        let off = addr - HARD_PARAM_BASE;
        let (i, rel) = (off / HARD_PARAM_STRIDE, off % HARD_PARAM_STRIDE);
        if i <= 31 && rel + count <= HARD_PARAM_BLOCK_WORDS {
            return Ok(());
        }
    }
    Err(Forbidden::ReadNotAllowed(addr, count))
}

/// Why a write to a plain register (not 0x65 / 0x66 / 0x67) is refused. This crate has no
/// representation for any plain register write, so this only classifies the reason.
pub(crate) fn plain_write_reason(addr: u32) -> Forbidden {
    if addr == 150 || addr == 151 {
        Forbidden::Register150Or151(addr)
    } else if (LICENCE_FIRST..=LICENCE_LAST).contains(&addr) {
        Forbidden::LicenceArea(addr)
    } else if addr >= HARD_PARAM_BASE {
        Forbidden::HardwareParameters(addr)
    } else if (11000..11000 + 2 * 39).contains(&addr) {
        Forbidden::ZfProperty(addr)
    } else if addr == 100 {
        Forbidden::Register100
    } else if (5000..=5012).contains(&addr) {
        Forbidden::Block5000(addr)
    } else if addr == 200 || addr == 201 {
        Forbidden::TestData(addr)
    } else {
        Forbidden::NotInAllowList(addr)
    }
}

/// Word indices of the system block at 50000 that the PC software uses. The card is
/// authoritative for all three.
pub mod system {
    /// Bus (interpolation) cycle in microseconds; the tick period of the motion queue.
    pub const BUS_CYCLE: usize = 5;
    /// Height-follower type; 0 means the machine has no on-board follower.
    pub const ZF_TYPE: usize = 6;
    /// Card units per millimetre (1000 on the owner's machine).
    pub const K: usize = 17;
}

/// Typed view of the system block at 50000 (26 words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemBlock {
    /// All words as read.
    pub words: Vec<u32>,
}

impl SystemBlock {
    /// Wrap a block read of at least 26 words.
    pub fn from_words(words: &[u32]) -> Option<SystemBlock> {
        (words.len() >= 26).then(|| SystemBlock { words: words.to_vec() })
    }

    /// Bus cycle in microseconds (register 50005).
    pub fn bus_cycle_us(&self) -> u32 {
        self.words[system::BUS_CYCLE]
    }

    /// Follower type (register 50006).
    pub fn zf_type(&self) -> u32 {
        self.words[system::ZF_TYPE]
    }

    /// True when the card reports an on-board height follower.
    pub fn has_follower(&self) -> bool {
        self.zf_type() != 0
    }

    /// Card units per millimetre (register 50017). `None` when the card reports 0, which no
    /// conversion can use.
    pub fn k(&self) -> Option<u32> {
        Some(self.words[system::K]).filter(|&k| k != 0)
    }
}

/// Word index inside one axis slot of block 2000. Words 3 to 9 exist but their names are not
/// confirmed, so they have no variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum AxisRo {
    /// Status word, see [`axis`].
    Status = 0,
    /// Speed in card units per second (0.001 mm/s at K = 1000).
    Speed = 1,
    /// Commanded position in pulses.
    PulsePosition = 2,
}

/// Typed view of the axis read-only block at 2000 (5 slots of 10 words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxisRoBlock {
    /// All 50 words as read.
    pub words: Vec<u32>,
}

impl AxisRoBlock {
    /// Wrap a block read of at least 50 words.
    pub fn from_words(words: &[u32]) -> Option<AxisRoBlock> {
        (words.len() >= AXIS_SLOTS * AXIS_RO_WORDS).then(|| AxisRoBlock { words: words[..AXIS_SLOTS * AXIS_RO_WORDS].to_vec() })
    }

    /// Raw word `word` (0..10) of `slot` (0..5), 0 when out of range.
    pub fn raw(&self, slot: usize, word: usize) -> u32 {
        if slot < AXIS_SLOTS && word < AXIS_RO_WORDS { self.words[AXIS_RO_WORDS * slot + word] } else { 0 }
    }

    /// Status word of `slot`.
    pub fn status(&self, slot: usize) -> u32 {
        self.raw(slot, AxisRo::Status as usize)
    }

    /// Signed speed word of `slot`.
    pub fn speed(&self, slot: usize) -> i32 {
        self.raw(slot, AxisRo::Speed as usize) as i32
    }

    /// Signed pulse position of `slot`.
    pub fn pulse_position(&self, slot: usize) -> i32 {
        self.raw(slot, AxisRo::PulsePosition as usize) as i32
    }

    /// True when the homed bit of `slot` is set.
    pub fn is_homed(&self, slot: usize) -> bool {
        self.status(slot) & axis::HOMED != 0
    }

    /// Five-bit mask of the homed slots.
    pub fn homed_mask(&self) -> u32 {
        (0..AXIS_SLOTS).filter(|&s| self.is_homed(s)).fold(0, |m, s| m | (1 << s))
    }

    /// True while `slot` executes a command.
    pub fn busy(&self, slot: usize) -> bool {
        axis::busy(self.status(slot))
    }

    /// The fault bits of `slot` (limits, servo, dual drive).
    pub fn faults(&self, slot: usize) -> u32 {
        self.status(slot) & axis::FAULT_MASK
    }
}

/// Word index inside one axis slot of block 50200. Only [`AxisRw::Lead`] and
/// [`AxisRw::CmdPulsesPerUnit`] have evidence; the other names follow the order of the vendor's
/// table and are unconfirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum AxisRw {
    /// Configuration word.
    Config = 0,
    /// Soft limit, positive side.
    SoftLimitPos = 1,
    /// Soft limit, negative side.
    SoftLimitNeg = 2,
    /// Acceleration.
    Accel = 3,
    /// Jerk.
    Jerk = 4,
    /// Homing speed, coarse pass.
    HomeCoarseSpeed = 5,
    /// Homing speed, fine pass.
    HomeFineSpeed = 6,
    /// Origin offset.
    OriginOffset = 7,
    /// Not used.
    Spare = 8,
    /// Screw lead times K.
    Lead = 9,
    /// Command pulses per revolution.
    CmdPulsesPerUnit = 10,
    /// Encoder pulses per revolution.
    EncPulsesPerUnit = 11,
    /// Input configuration.
    InputCfg = 12,
    /// Output configuration.
    OutputCfg = 13,
}

/// Words per axis slot the vendor software keeps from block 50200 (and that the fast block carries).
pub const AXIS_RW_WORDS_KEPT: usize = 14;

/// Typed view of the axis read/write block at 50200 (5 slots of 20 words). Read only: this crate
/// has no write for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxisRwBlock {
    /// All 100 words as read.
    pub words: Vec<u32>,
}

impl AxisRwBlock {
    /// Wrap a block read of at least 100 words.
    pub fn from_words(words: &[u32]) -> Option<AxisRwBlock> {
        let n = AXIS_SLOTS * AXIS_RW_WORDS_ON_WIRE;
        (words.len() >= n).then(|| AxisRwBlock { words: words[..n].to_vec() })
    }

    /// Word `w` of `slot`, 0 when the slot is out of range.
    pub fn word(&self, slot: usize, w: AxisRw) -> u32 {
        if slot < AXIS_SLOTS { self.words[AXIS_RW_WORDS_ON_WIRE * slot + w as usize] } else { 0 }
    }

    /// Command pulses per millimetre of `slot`: `pulses per revolution * K / (lead * K)`. `None`
    /// when the card reports no lead.
    pub fn pulses_per_mm(&self, slot: usize, k: u32) -> Option<f64> {
        pulses_per_mm(self.word(slot, AxisRw::Lead), self.word(slot, AxisRw::CmdPulsesPerUnit), k)
    }
}

fn pulses_per_mm(lead_word: u32, pulses_word: u32, k: u32) -> Option<f64> {
    (lead_word != 0 && k != 0).then(|| pulses_word as f64 * k as f64 / lead_word as f64)
}

/// Typed view of the fast block at 60001: the axis read-only block (words 0..50) followed by the
/// 14 kept words of every axis slot of block 50200 (words 50..120), in one read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FastBlock {
    /// All words as read (at least 120).
    pub words: Vec<u32>,
}

impl FastBlock {
    /// Wrap a block read of at least 120 words.
    pub fn from_words(words: &[u32]) -> Option<FastBlock> {
        (words.len() >= 120).then(|| FastBlock { words: words.to_vec() })
    }

    /// The axis read-only part.
    pub fn axis_ro(&self) -> AxisRoBlock {
        AxisRoBlock { words: self.words[..AXIS_SLOTS * AXIS_RO_WORDS].to_vec() }
    }

    /// Word `w` of the read/write part of `slot`, 0 when the slot is out of range.
    pub fn axis_rw(&self, slot: usize, w: AxisRw) -> u32 {
        if slot < AXIS_SLOTS { self.words[AXIS_SLOTS * AXIS_RO_WORDS + AXIS_RW_WORDS_KEPT * slot + w as usize] } else { 0 }
    }

    /// Command pulses per millimetre of `slot`, like [`AxisRwBlock::pulses_per_mm`].
    pub fn pulses_per_mm(&self, slot: usize, k: u32) -> Option<f64> {
        pulses_per_mm(self.axis_rw(slot, AxisRw::Lead), self.axis_rw(slot, AxisRw::CmdPulsesPerUnit), k)
    }
}

/// Start register of hardware-parameter block `index` (0..=31): `59600 + 0xD0 * index`, 52 words.
/// These blocks are read to compare them with the PC's copy; this crate never writes them.
pub fn hard_param_block_addr(index: u32) -> Option<u32> {
    (index <= 31).then(|| HARD_PARAM_BASE + HARD_PARAM_STRIDE * index)
}

/// Period of the vendor's core loop in milliseconds: the status block is read once per loop.
pub const VENDOR_POLL_MS: u32 = 30;
/// A status poll that fails for this long during a job means: stop the stream and disarm.
pub const STATUS_WATCHDOG_MS: u32 = 1000;

/// One read of a polling schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollEntry {
    /// Name of the block in [`BLOCKS`].
    pub block: &'static str,
    /// Period in milliseconds.
    pub period_ms: u32,
    /// True for the fast queue, which slow reads never hold up.
    pub fast: bool,
    /// True when the read only happens on a machine with an on-board follower.
    pub needs_follower: bool,
}

impl PollEntry {
    /// The `(start, count)` of the read, if the block is known.
    pub fn read(&self) -> Option<(u32, u32)> {
        block(self.block).map(|b| (b.start, b.count))
    }
}

/// Blocks the vendor reads every cycle in normal mode.
pub const POLL_PROFILE_NORMAL: &[&str] = &["status", "axis_ro", "axis_rw", "system_rw"];
/// Blocks the vendor reads every cycle in fast mode.
pub const POLL_PROFILE_FAST: &[&str] = &["status", "system_rw", "fast"];

/// Polling schedule of the owner's port. Only the 30 ms status period is the vendor's; the other
/// periods are the port's own choice and unconfirmed against the card.
pub const POLL_SCHEDULE: &[PollEntry] = &[
    PollEntry { block: "status", period_ms: VENDOR_POLL_MS, fast: true, needs_follower: false },
    PollEntry { block: "axis_ro", period_ms: 3 * VENDOR_POLL_MS, fast: true, needs_follower: false },
    PollEntry { block: "system_rw", period_ms: 1000, fast: false, needs_follower: false },
    PollEntry { block: "axis_rw", period_ms: 1000, fast: false, needs_follower: false },
    PollEntry { block: "zf_status", period_ms: 10_000, fast: false, needs_follower: true },
];

/// The schedule entries due at `now_ms`, given when each was last read (`None` = never). Entries
/// that need a follower are skipped when `has_follower` is false.
pub fn polls_due(now_ms: u64, last_read_ms: &[Option<u64>], has_follower: bool) -> Vec<&'static PollEntry> {
    POLL_SCHEDULE
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            (has_follower || !e.needs_follower)
                && last_read_ms.get(*i).copied().flatten().is_none_or(|t| now_ms.saturating_sub(t) >= e.period_ms as u64)
        })
        .map(|(_, e)| e)
        .collect()
}

/// Facts the owner's analysis marks as inferred or unverified.
pub const UNVERIFIED_NOTES: &[&str] = &[
    "register 1016 unit (bytes free assumed, 60000 = empty)",
    "whether register 1004 is the raw level or already inverted by the card",
    "axis RO words 3..9 names, axis RW word names other than 9 and 10",
    "alarm1 bit 24 meaning (follower axis, low-medium)",
    "axis command type split between jog and move (3, 4, 5)",
    "FIFO tick period (register 50005 default 250 us)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_addresses() {
        assert_eq!(Status::Alarm1.addr(), 1006);
        assert_eq!(Status::FifoFrameId.addr(), 1015);
        assert_eq!(Status::FifoSpaceMargin.addr(), 1016);
        assert_eq!(Status::ProcessingStatus.addr(), 1019);
        assert_eq!(Status::ParamStatus.addr(), 1031);
    }

    #[test]
    fn blocks_are_consistent() {
        assert!(block("zf_status").is_some_and(|b| b.start == 10000 && b.count == 18));
        assert!(block("status").is_some_and(|b| b.contains(1000, 36) && !b.contains(1000, 37)));
        assert!(BLOCKS.iter().all(|b| b.count > 0));
        // No block overlaps the licence area.
        assert!(BLOCKS.iter().all(|b| b.end() <= LICENCE_FIRST || b.start > LICENCE_LAST));
    }

    #[test]
    fn reads_follow_the_allow_list() {
        assert!(check_read(1000, 2).is_ok());
        assert!(check_read(1000, 36).is_ok());
        assert!(check_read(10000, 18).is_ok());
        assert!(check_read(1000, 40).is_err());
        assert!(check_read(0, 1).is_err());
        assert!(check_read(59600, 52).is_ok());
        assert!(check_read(59600 + 0xD0, 52).is_ok());
        assert!(check_read(59600, 53).is_err());
        assert!(matches!(check_read(59500, 1), Err(Forbidden::LicenceArea(_))));
        assert!(matches!(check_read(59502, 2), Err(Forbidden::LicenceArea(_))));
        assert!(matches!(check_read(150, 1), Err(Forbidden::Register150Or151(_))));
        assert!(matches!(check_read(140, 20), Err(Forbidden::Register150Or151(_))));
        assert!(matches!(check_read(200, 1), Err(Forbidden::TestData(_))));
        assert!(check_read(1000, 0).is_err());
    }

    #[test]
    fn status_block_decoding() {
        let mut w = vec![0u32; 36];
        w[Status::ProcessingStatus as usize] = 0x0101;
        w[Status::Do as usize] = 1 << 4;
        w[Status::FifoSpaceMargin as usize] = FIFO_MARGIN_EMPTY;
        w[Status::ParamStatus as usize] = 1;
        let s = StatusBlock::from_words(&w).unwrap();
        assert!(s.fifo_running());
        assert!(s.do_is_on(5));
        assert!(!s.do_is_on(6));
        assert!(!s.do_is_on(0));
        assert!(s.restart_required());
        assert_eq!(s.fifo_space_margin(), 60000);
        assert!(StatusBlock::from_words(&w[..31]).is_none());
    }

    #[test]
    fn machine_state_priority() {
        let mut w = vec![0u32; 36];
        let mut axis_ro = vec![0u32; 50];
        let s = StatusBlock::from_words(&w).unwrap();
        assert_eq!(derive_machine_state(&s, &axis_ro), MachineState::Ready);
        axis_ro[10] = (1 << 16) | (2 << 24);
        assert_eq!(derive_machine_state(&s, &axis_ro), MachineState::Origin);
        axis_ro[0] = (1 << 16) | (3 << 24);
        assert_eq!(derive_machine_state(&s, &axis_ro), MachineState::Jog);
        w[Status::ProcessingStatus as usize] = 1;
        let s = StatusBlock::from_words(&w).unwrap();
        assert_eq!(derive_machine_state(&s, &axis_ro), MachineState::Process);
    }

    #[test]
    fn alarm_decoding() {
        let codes = alarm_codes(alarm1::ESTOP | alarm1::AXIS_X, alarm2::FIFO_STARVATION, None);
        let c: Vec<u32> = codes.iter().map(|a| a.code).collect();
        assert_eq!(c, vec![8000, 8030, 9005]);
        let mut ro = vec![0u32; 50];
        ro[0] = 0b101;
        let c: Vec<u32> = alarm_codes(alarm1::AXIS_X, 0, Some(&ro)).iter().map(|a| a.code).collect();
        assert_eq!(c, vec![8100, 8102]);
        assert_eq!(alarm_codes(0, 0x40, None)[0].code, 9099);
        assert_eq!(alarm_codes(1 << 27, 0, None)[0].label, "unknown");
    }

    #[test]
    fn zf_state_decoding() {
        let mut w = vec![0u32; 18];
        let z = |w: &Vec<u32>| ZfStatus::from_words(w).unwrap().state();
        assert_eq!(z(&w), ZfState::Estop);
        w[2] = 0x8000_0000;
        assert_eq!(z(&w), ZfState::Ready);
        w[3] = 104;
        assert_eq!(z(&w), ZfState::Follow);
        w[2] = 0x8000_0004;
        assert_eq!(z(&w), ZfState::Drill);
        w[1] = 0x1_0000 | zf_alarm::FOLLOW_ERROR;
        assert_eq!(ZfStatus::from_words(&w).unwrap().alarm(), zf_alarm::FOLLOW_ERROR);
    }

    #[test]
    fn axis_addresses() {
        assert_eq!(axis::ro_addr(1, 2), Some(2012));
        assert_eq!(axis::ro_addr(5, 0), None);
        assert_eq!(axis::rw_addr(2, 9), Some(50249));
        assert!(axis::busy(1 << 16));
        assert!(!axis::busy(axis::HOMED));
    }

    #[test]
    fn plain_writes_have_reasons() {
        assert_eq!(plain_write_reason(150), Forbidden::Register150Or151(150));
        assert_eq!(plain_write_reason(151), Forbidden::Register150Or151(151));
        assert!(matches!(plain_write_reason(59510), Forbidden::LicenceArea(_)));
        assert!(matches!(plain_write_reason(59700), Forbidden::HardwareParameters(_)));
        assert!(matches!(plain_write_reason(11002), Forbidden::ZfProperty(_)));
        assert!(matches!(plain_write_reason(5008), Forbidden::Block5000(_)));
        assert!(matches!(plain_write_reason(3), Forbidden::NotInAllowList(3)));
    }

    #[test]
    fn version_words_decode_and_are_compared_with_the_minimum() {
        let v = VersionInfo::from_words(&[100, 20152]).unwrap();
        assert_eq!((v.program_id, v.program_version), (100, 20152));
        assert!(v.meets(MIN_PROGRAM_VERSION));
        assert!(!v.meets(MIN_PROGRAM_VERSION_SP1));
        assert!(VersionInfo::from_words(&[100]).is_none());
        let mut w = vec![0u32; 36];
        w[1] = 20177;
        assert!(StatusBlock::from_words(&w).unwrap().version().meets(MIN_PROGRAM_VERSION_SP1));
    }

    #[test]
    fn inputs_follow_the_input_type_and_port_zero_is_never_active() {
        let mut w = vec![0u32; 36];
        w[Status::Di as usize] = 0b101;
        w[Status::ExtDi as usize] = 0b10;
        w[Status::FifoFrameId as usize] = u32::MAX;
        let s = StatusBlock::from_words(&w).unwrap();
        assert!(s.di_active(1, false) && !s.di_active(2, false) && s.di_active(3, false));
        assert!(!s.di_active(1, true) && s.di_active(2, true));
        assert!(s.di_active(14, false) && !s.di_active(13, false));
        assert!(!s.di_active(0, true) && !s.di_active(0, false) && !s.di_active(29, true));
        assert_eq!(s.next_frame_id(), 0, "the frame id wraps");
    }

    #[test]
    fn alarm_words_are_named() {
        let l = alarm_labels(alarm1::AXIS_Y | alarm1::ESTOP | (1 << 27), alarm2::FIFO_STARVATION | (1 << 9));
        assert_eq!(
            l,
            ["axis Y fault", "alarm 1 bit 27 (no known meaning)", "emergency stop", "motion queue ran empty", "alarm 2 bit 9 (no known meaning)"]
        );
        assert!(alarm_labels(0, 0).is_empty());
        assert_eq!(axis::fault_names(axis::SOFT_LIMIT_NEG | axis::SERVO_ALARM | axis::HOMED), ["soft limit -", "servo alarm"]);
        assert_eq!(zf_alarm::names(zf_alarm::TOUCH_PLATE | zf_alarm::FTC_ALARM), ["touched the plate", "follower alarm"]);
    }

    #[test]
    fn system_block_gives_cycle_follower_type_and_k() {
        let mut w = vec![0u32; 26];
        (w[5], w[6], w[17]) = (250, 1, 1000);
        let s = SystemBlock::from_words(&w).unwrap();
        assert_eq!((s.bus_cycle_us(), s.zf_type(), s.k()), (250, 1, Some(1000)));
        assert!(s.has_follower());
        w[17] = 0;
        w[6] = 0;
        let s = SystemBlock::from_words(&w).unwrap();
        assert_eq!(s.k(), None);
        assert!(!s.has_follower());
        assert!(SystemBlock::from_words(&w[..25]).is_none());
    }

    #[test]
    fn axis_blocks_decode_per_slot() {
        let mut ro = vec![0u32; 50];
        ro[0] = axis::HOMED | axis::SOFT_LIMIT_POS;
        ro[1] = (-2500i32) as u32;
        ro[2] = 123_456;
        ro[20] = axis::HOMED | (1 << 16) | (3 << 24);
        ro[22] = (-7i32) as u32;
        let b = AxisRoBlock::from_words(&ro).unwrap();
        assert_eq!(b.homed_mask(), 0b101);
        assert_eq!((b.speed(0), b.pulse_position(0), b.pulse_position(2)), (-2500, 123_456, -7));
        assert_eq!(b.faults(0), axis::SOFT_LIMIT_POS);
        assert!(b.busy(2) && !b.busy(0));
        assert_eq!(b.status(9), 0, "a slot that does not exist reads as 0");
        assert!(AxisRoBlock::from_words(&ro[..49]).is_none());

        let mut rw = vec![0u32; 100];
        rw[9] = 10_000; // lead 10 mm at K = 1000
        rw[10] = 10_000; // pulses per revolution
        rw[20 + 9] = 5_000;
        rw[20 + 10] = 10_000;
        let b = AxisRwBlock::from_words(&rw).unwrap();
        assert_eq!(b.word(1, AxisRw::Lead), 5_000);
        assert_eq!(b.pulses_per_mm(0, 1000), Some(1000.0));
        assert_eq!(b.pulses_per_mm(1, 1000), Some(2000.0));
        assert_eq!(b.pulses_per_mm(2, 1000), None, "no lead reported");
        assert_eq!(b.pulses_per_mm(0, 0), None);
        assert!(AxisRwBlock::from_words(&rw[..99]).is_none());
    }

    #[test]
    fn the_fast_block_is_the_axis_block_plus_fourteen_words_per_slot() {
        let mut f = vec![0u32; 120];
        f[10] = axis::HOMED;
        f[50 + 14 + 9] = 4_000;
        f[50 + 14 + 10] = 8_000;
        let b = FastBlock::from_words(&f).unwrap();
        assert_eq!(b.axis_ro().homed_mask(), 0b10);
        assert_eq!(b.axis_rw(1, AxisRw::CmdPulsesPerUnit), 8_000);
        assert_eq!(b.pulses_per_mm(1, 1000), Some(2000.0));
        assert!(FastBlock::from_words(&f[..119]).is_none());
    }

    #[test]
    fn follower_words_have_names() {
        let mut w = vec![0u32; 18];
        w[1] = zf_alarm::ENCODER | 0xFFFF_0000;
        w[6] = 42;
        let z = ZfStatus::from_words(&w).unwrap();
        assert_eq!(z.alarm_names(), ["encoder"]);
        assert_eq!(z.word(ZfWord::ZPosition), 42);
        assert_eq!(ZfWord::Other as usize, 17);
    }

    #[test]
    fn hardware_parameter_blocks_are_readable_and_never_writable() {
        assert_eq!(hard_param_block_addr(0), Some(59600));
        assert_eq!(hard_param_block_addr(31), Some(59600 + 0xD0 * 31));
        assert_eq!(hard_param_block_addr(32), None);
        for i in 0..32 {
            let a = hard_param_block_addr(i).unwrap();
            assert!(check_read(a, 52).is_ok(), "block {i}");
            assert!(matches!(plain_write_reason(a), Forbidden::HardwareParameters(_)));
        }
        // one word past a block is not part of it (block 2 lies inside the fast block's range,
        // so the first and the last block are the ones to ask)
        assert!(check_read(59600, 53).is_err());
        assert!(check_read(hard_param_block_addr(31).unwrap(), 53).is_err());
    }

    #[test]
    fn poll_schedule_names_known_blocks() {
        for e in POLL_SCHEDULE {
            assert!(e.read().is_some(), "{}", e.block);
        }
        assert_eq!(POLL_SCHEDULE[0].read(), Some((1000, 36)));
        for name in POLL_PROFILE_NORMAL.iter().chain(POLL_PROFILE_FAST) {
            assert!(block(name).is_some(), "{name}");
        }
        // nothing read yet: everything is due, the follower block only with a follower
        assert_eq!(polls_due(0, &[], true).len(), 5);
        assert_eq!(polls_due(0, &[], false).len(), 4);
        // 40 ms after reading everything only the status block is due again
        let last = [Some(0u64); 5];
        let due: Vec<_> = polls_due(40, &last, true).iter().map(|e| e.block).collect();
        assert_eq!(due, ["status"]);
        let due: Vec<_> = polls_due(1000, &last, true).iter().map(|e| e.block).collect();
        assert_eq!(due, ["status", "axis_ro", "system_rw", "axis_rw"]);
    }
}
