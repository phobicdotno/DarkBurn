//! Controller memory addresses ("settings registers") and the machine-settings backup.
//!
//! Addresses are the 14-bit values sent after `DA 00` / `DA 01` (for example `05 7E` on the
//! wire is address 0x2FE). Names and addresses follow MeerK40t's emulator memory table (MIT)
//! and EduTechWiki. Many are only partly verified; [`Register::verified`] marks the ones that
//! several sources agree on. Everything else needs checking on a real controller.

use std::collections::BTreeMap;

/// What a register value means, for display and conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Unit {
    /// Length in µm.
    Micron,
    /// Speed in µm/s.
    MicronPerSecond,
    /// Acceleration in µm/s².
    MicronPerSecond2,
    /// Time in µs.
    Microsecond,
    /// Time in ms.
    Millisecond,
    /// Time in seconds.
    Second,
    /// Frequency in Hz.
    Hertz,
    /// Power or ratio in 0.1 % steps. UNCERTAIN for the laser power registers: job power is
    /// 14-bit (raw / 163.84 = percent), and no source gives the unit of the registers.
    Permille,
    /// Percent.
    Percent,
    /// Bit field.
    Bits,
    /// Plain count or identifier.
    Count,
}

/// Which settings page a register belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Group {
    Laser,
    Laser2,
    XAxis,
    YAxis,
    ZAxis,
    UAxis,
    Rotary,
    Motion,
    System,
    Status,
    Info,
}

/// One known controller memory address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Register {
    pub addr: u16,
    pub name: &'static str,
    pub unit: Unit,
    pub group: Group,
    /// Writable machine setting (included in backups).
    pub writable: bool,
    /// Agreed by more than one public source.
    pub verified: bool,
}

const fn r(addr: u16, name: &'static str, unit: Unit, group: Group, writable: bool, verified: bool) -> Register {
    Register { addr, name, unit, group, writable, verified }
}

/// Well-known addresses as constants.
pub mod addr {
    pub const G0_VELOCITY: u16 = 0x0005;
    pub const HOME_VELOCITY: u16 = 0x000C;
    pub const LASER1_PWM_FREQ: u16 = 0x0011;
    pub const LASER1_MIN_POWER: u16 = 0x0012;
    pub const LASER1_MAX_POWER: u16 = 0x0013;
    pub const LASER2_PWM_FREQ: u16 = 0x0017;
    pub const LASER2_MIN_POWER: u16 = 0x0018;
    pub const LASER2_MAX_POWER: u16 = 0x0019;
    pub const LASER1_STANDBY_FREQ: u16 = 0x001A;
    pub const LASER1_STANDBY_PULSE: u16 = 0x001B;
    pub const LASER2_STANDBY_FREQ: u16 = 0x001C;
    pub const LASER2_STANDBY_PULSE: u16 = 0x001D;
    pub const X_CONTROL: u16 = 0x0020;
    pub const X_PRECISION: u16 = 0x0021;
    pub const X_MAX_VELOCITY: u16 = 0x0023;
    pub const X_START_VELOCITY: u16 = 0x0024;
    pub const X_MAX_ACC: u16 = 0x0025;
    pub const BED_X: u16 = 0x0026;
    pub const X_HOME_OFFSET: u16 = 0x002A;
    pub const X_BACKLASH: u16 = 0x002B;
    pub const Y_CONTROL: u16 = 0x0030;
    pub const Y_PRECISION: u16 = 0x0031;
    pub const Y_MAX_VELOCITY: u16 = 0x0033;
    pub const Y_START_VELOCITY: u16 = 0x0034;
    pub const Y_MAX_ACC: u16 = 0x0035;
    pub const BED_Y: u16 = 0x0036;
    pub const Y_HOME_OFFSET: u16 = 0x003A;
    pub const Y_BACKLASH: u16 = 0x003B;
    pub const Z_CONTROL: u16 = 0x0040;
    pub const Z_PRECISION: u16 = 0x0041;
    pub const Z_MAX_VELOCITY: u16 = 0x0043;
    pub const Z_RANGE: u16 = 0x0046;
    pub const U_CONTROL: u16 = 0x0050;
    pub const U_PRECISION: u16 = 0x0051;
    pub const U_MAX_VELOCITY: u16 = 0x0053;
    pub const U_START_VELOCITY: u16 = 0x0054;
    pub const U_MAX_ACC: u16 = 0x0055;
    pub const U_RANGE: u16 = 0x0056;
    pub const U_HOME_OFFSET: u16 = 0x005A;
    pub const U_BACKLASH: u16 = 0x005B;
    pub const SYSTEM_SETTINGS: u16 = 0x0100;
    pub const TURN_VELOCITY: u16 = 0x0101;
    pub const SYN_ACC: u16 = 0x0102;
    pub const G0_ACC: u16 = 0x010A;
    pub const FOCUS_DEPTH: u16 = 0x010E;
    pub const ROTATE_OFF_DELAY: u16 = 0x0119;
    pub const ROTATE_PULSE: u16 = 0x011F;
    pub const ROTATE_DIAMETER: u16 = 0x0121;
    pub const X_MIN_ENG_VELOCITY: u16 = 0x0124;
    pub const X_ENG_ACC: u16 = 0x0125;
    pub const Z_HOME_VELOCITY: u16 = 0x0128;
    pub const Z_WORK_VELOCITY: u16 = 0x0129;
    pub const U_HOME_VELOCITY: u16 = 0x012C;
    pub const U_WORK_VELOCITY: u16 = 0x012D;
    pub const MANUAL_FAST_SPEED: u16 = 0x0131;
    pub const MANUAL_SLOW_SPEED: u16 = 0x0132;
    pub const Y_MIN_ENG_VELOCITY: u16 = 0x0134;
    pub const Y_ENG_ACC: u16 = 0x0135;
    pub const MACHINE_STATUS: u16 = 0x0200;
    pub const TOTAL_OPEN_TIME: u16 = 0x0201;
    pub const TOTAL_WORK_TIME: u16 = 0x0202;
    pub const TOTAL_WORK_NUMBER: u16 = 0x0203;
    pub const FILE_COUNT: u16 = 0x0205;
    pub const PREVIOUS_WORK_TIME: u16 = 0x0208;
    pub const TOTAL_LASER_ON_TIME: u16 = 0x0211;
    pub const POS_X: u16 = 0x0221;
    pub const POS_Y: u16 = 0x0231;
    pub const POS_Z: u16 = 0x0241;
    pub const POS_U: u16 = 0x0251;
    pub const CARD_ID: u16 = 0x02FE;
    pub const FIRMWARE: u16 = 0x02FF;
    /// Run time of stored file n is at `FILE_TIME_BASE + n` (n is 1-based).
    pub const FILE_TIME_BASE: u16 = 0x0390;
    pub const CARD_LOCK: u16 = 0x0591;
}

use Group::*;
use Unit::*;

/// The register table. Writable entries form the machine-settings backup.
pub const REGISTERS: &[Register] = &[
    r(addr::G0_VELOCITY, "Idle (G0) speed", MicronPerSecond, Motion, true, false),
    r(addr::HOME_VELOCITY, "Homing speed", MicronPerSecond, Motion, true, false),
    r(addr::LASER1_PWM_FREQ, "Laser 1 PWM frequency", Hertz, Laser, true, false),
    r(addr::LASER1_MIN_POWER, "Laser 1 minimum power", Permille, Laser, true, false),
    r(addr::LASER1_MAX_POWER, "Laser 1 maximum power", Permille, Laser, true, false),
    r(addr::LASER1_STANDBY_FREQ, "Laser 1 standby frequency", Hertz, Laser, true, false),
    r(addr::LASER1_STANDBY_PULSE, "Laser 1 standby pulse", Microsecond, Laser, true, false),
    r(addr::LASER2_PWM_FREQ, "Laser 2 PWM frequency", Hertz, Laser2, true, false),
    r(addr::LASER2_MIN_POWER, "Laser 2 minimum power", Permille, Laser2, true, false),
    r(addr::LASER2_MAX_POWER, "Laser 2 maximum power", Permille, Laser2, true, false),
    r(addr::LASER2_STANDBY_FREQ, "Laser 2 standby frequency", Hertz, Laser2, true, false),
    r(addr::LASER2_STANDBY_PULSE, "Laser 2 standby pulse", Microsecond, Laser2, true, false),
    r(addr::X_CONTROL, "X control flags", Bits, XAxis, true, false),
    r(addr::X_PRECISION, "X step length", Micron, XAxis, true, false),
    r(addr::X_MAX_VELOCITY, "X maximum speed", MicronPerSecond, XAxis, true, false),
    r(addr::X_START_VELOCITY, "X start speed", MicronPerSecond, XAxis, true, false),
    r(addr::X_MAX_ACC, "X maximum acceleration", MicronPerSecond2, XAxis, true, false),
    r(addr::BED_X, "X travel (bed width)", Micron, XAxis, true, true),
    r(addr::X_HOME_OFFSET, "X home offset", Micron, XAxis, true, false),
    r(addr::X_BACKLASH, "X backlash", Micron, XAxis, true, false),
    r(addr::Y_CONTROL, "Y control flags", Bits, YAxis, true, false),
    r(addr::Y_PRECISION, "Y step length", Micron, YAxis, true, false),
    r(addr::Y_MAX_VELOCITY, "Y maximum speed", MicronPerSecond, YAxis, true, false),
    r(addr::Y_START_VELOCITY, "Y start speed", MicronPerSecond, YAxis, true, false),
    r(addr::Y_MAX_ACC, "Y maximum acceleration", MicronPerSecond2, YAxis, true, false),
    r(addr::BED_Y, "Y travel (bed height)", Micron, YAxis, true, true),
    r(addr::Y_HOME_OFFSET, "Y home offset", Micron, YAxis, true, false),
    r(addr::Y_BACKLASH, "Y backlash", Micron, YAxis, true, false),
    r(addr::Z_CONTROL, "Z control flags", Bits, ZAxis, true, false),
    r(addr::Z_PRECISION, "Z step length", Micron, ZAxis, true, false),
    r(addr::Z_MAX_VELOCITY, "Z maximum speed", MicronPerSecond, ZAxis, true, false),
    r(addr::Z_RANGE, "Z travel", Micron, ZAxis, true, false),
    r(addr::Z_HOME_VELOCITY, "Z homing speed", MicronPerSecond, ZAxis, true, false),
    r(addr::Z_WORK_VELOCITY, "Z work speed", MicronPerSecond, ZAxis, true, false),
    r(addr::U_CONTROL, "U control flags", Bits, UAxis, true, false),
    r(addr::U_PRECISION, "U step length", Micron, UAxis, true, false),
    r(addr::U_MAX_VELOCITY, "U maximum speed", MicronPerSecond, UAxis, true, false),
    r(addr::U_START_VELOCITY, "U start speed", MicronPerSecond, UAxis, true, false),
    r(addr::U_MAX_ACC, "U maximum acceleration", MicronPerSecond2, UAxis, true, false),
    r(addr::U_RANGE, "U travel", Micron, UAxis, true, false),
    r(addr::U_HOME_OFFSET, "U home offset", Micron, UAxis, true, false),
    r(addr::U_BACKLASH, "U backlash", Micron, UAxis, true, false),
    r(addr::U_HOME_VELOCITY, "U homing speed", MicronPerSecond, UAxis, true, false),
    r(addr::U_WORK_VELOCITY, "U work speed", MicronPerSecond, UAxis, true, false),
    r(addr::ROTATE_PULSE, "Rotary steps per rotation", Count, Rotary, true, false),
    r(addr::ROTATE_DIAMETER, "Rotary roller diameter", Micron, Rotary, true, false),
    r(addr::ROTATE_OFF_DELAY, "Rotary off delay", Millisecond, Rotary, true, false),
    r(addr::SYSTEM_SETTINGS, "System flags (return position)", Bits, System, true, false),
    r(addr::TURN_VELOCITY, "Corner speed", MicronPerSecond, Motion, true, false),
    r(addr::SYN_ACC, "Cut acceleration", MicronPerSecond2, Motion, true, false),
    r(addr::G0_ACC, "Idle acceleration", MicronPerSecond2, Motion, true, false),
    r(addr::FOCUS_DEPTH, "Focus distance", Micron, ZAxis, true, false),
    r(addr::X_MIN_ENG_VELOCITY, "X minimum engraving speed", MicronPerSecond, Motion, true, false),
    r(addr::X_ENG_ACC, "X engraving acceleration", MicronPerSecond2, Motion, true, false),
    r(addr::Y_MIN_ENG_VELOCITY, "Y minimum engraving speed", MicronPerSecond, Motion, true, false),
    r(addr::Y_ENG_ACC, "Y engraving acceleration", MicronPerSecond2, Motion, true, false),
    r(addr::MANUAL_FAST_SPEED, "Jog fast speed", MicronPerSecond, Motion, true, false),
    r(addr::MANUAL_SLOW_SPEED, "Jog slow speed", MicronPerSecond, Motion, true, false),
    r(addr::MACHINE_STATUS, "Machine status", Bits, Status, false, false),
    r(addr::TOTAL_OPEN_TIME, "Total power-on time", Second, Info, false, false),
    r(addr::TOTAL_WORK_TIME, "Total job time", Second, Info, false, false),
    r(addr::TOTAL_WORK_NUMBER, "Jobs run", Count, Info, false, false),
    r(addr::FILE_COUNT, "Files stored", Count, Status, false, true),
    r(addr::PREVIOUS_WORK_TIME, "Last job time", Millisecond, Info, false, false),
    r(addr::TOTAL_LASER_ON_TIME, "Total laser-on time", Second, Info, false, false),
    r(addr::POS_X, "Current X", Micron, Status, false, true),
    r(addr::POS_Y, "Current Y", Micron, Status, false, true),
    r(addr::POS_Z, "Current Z", Micron, Status, false, true),
    r(addr::POS_U, "Current U", Micron, Status, false, true),
    r(addr::CARD_ID, "Card ID", Count, Info, false, true),
    r(addr::FIRMWARE, "Firmware version", Count, Info, false, true),
];

/// Look up a register by address.
pub fn register(addr: u16) -> Option<&'static Register> {
    REGISTERS.iter().find(|r| r.addr == addr)
}

/// All writable registers, optionally filtered by group.
pub fn writable(group: Option<Group>) -> impl Iterator<Item = &'static Register> {
    REGISTERS.iter().filter(move |r| r.writable && group.is_none_or(|g| r.group == g))
}

/// A controller board DarkBurn knows by name (RUIDA-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Board {
    /// Model or model series as printed on the board.
    pub model: &'static str,
    /// The card ID it answers at address 0x2FE, where a public source gives one.
    pub card_id: Option<u32>,
    /// Where the card ID comes from, or what is known instead.
    pub note: &'static str,
}

const fn b(model: &'static str, card_id: Option<u32>, note: &'static str) -> Board {
    Board { model, card_id, note }
}

/// The Ruida boards DarkBurn lists. All of them speak the same protocol family; only two card
/// IDs are public (MeerK40t and ruida-pa, both MIT), so every other board is accepted as a
/// generic Ruida controller and shown by its card ID. Needs checking on each board.
pub const KNOWN_BOARDS: &[Board] = &[
    b("RDC6442S", Some(0x6510_6510), "card ID from MeerK40t and ruida-pa (seen on a controller by ruida-pa's author)"),
    b("RDC6442G", Some(0x6500_6500), "card ID is the one MeerK40t's emulator answers with; not a reading from a board"),
    b("RDC8445S", Some(0x9010_9010), "card ID from ruida-pa"),
    b("RDC644x (6445G, 6445S)", None, "card ID not public"),
    b("RDC654x", None, "card ID not public"),
    b("RDC658x", None, "card ID not public"),
    b("RDLC-5121", None, "card ID not public"),
    b("RDLC320-A", None, "card ID not public"),
    b("RDC420B", None, "card ID not public"),
    b("RDC6332G", None, "card ID not public"),
    b("KT332N", None, "card ID not public"),
];

/// The board that answers with `card_id`, if a public source names it.
pub fn board(card_id: u32) -> Option<&'static Board> {
    KNOWN_BOARDS.iter().find(|b| b.card_id == Some(card_id))
}

/// Known board names by card ID. 0x65106510 (RDC6442S) and 0x90109010 (RDC8445S) are named by
/// public sources (MeerK40t, ruida-pa); 0x65006500 is only MeerK40t's emulator value.
pub fn board_name(card_id: u32) -> Option<&'static str> {
    board(card_id).map(|b| b.model)
}

/// What to call a controller: the board model when the card ID is known, otherwise a generic
/// name with the ID so the user can report it.
pub fn board_label(card_id: u32) -> String {
    match board_name(card_id) {
        Some(n) => n.to_owned(),
        None => format!("Ruida controller (card ID 0x{card_id:08X}, not in the board list)"),
    }
}

/// A full backup of the writable machine settings. Serialisable (JSON, RON, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MachineSettings {
    pub card_id: Option<u32>,
    pub firmware: Option<String>,
    /// Address -> raw value.
    pub values: BTreeMap<u16, u32>,
}

impl MachineSettings {
    pub fn get(&self, addr: u16) -> Option<u32> {
        self.values.get(&addr).copied()
    }

    pub fn set(&mut self, addr: u16, value: u32) {
        self.values.insert(addr, value);
    }

    /// Bed size in mm, if both axes are present.
    pub fn bed_size_mm(&self) -> Option<(f64, f64)> {
        Some((self.get(addr::BED_X)? as f64 / 1000.0, self.get(addr::BED_Y)? as f64 / 1000.0))
    }

    /// Addresses whose values differ from `other`, including ones missing on either side.
    pub fn diff(&self, other: &MachineSettings) -> Vec<u16> {
        let mut keys: Vec<u16> = self.values.keys().chain(other.values.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        keys.into_iter().filter(|k| self.values.get(k) != other.values.get(k)).collect()
    }
}

/// Laser 2 (dual tube) settings held in controller memory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Laser2Settings {
    pub pwm_freq_hz: u32,
    /// 0.1 % steps (MeerK40t calls these permille; UNCERTAIN).
    pub min_power: u32,
    pub max_power: u32,
    pub standby_freq_hz: u32,
    pub standby_pulse: u32,
}

impl Laser2Settings {
    pub const ADDRS: [u16; 5] =
        [addr::LASER2_PWM_FREQ, addr::LASER2_MIN_POWER, addr::LASER2_MAX_POWER, addr::LASER2_STANDBY_FREQ, addr::LASER2_STANDBY_PULSE];

    pub fn to_values(self) -> [u32; 5] {
        [self.pwm_freq_hz, self.min_power, self.max_power, self.standby_freq_hz, self.standby_pulse]
    }

    pub fn from_values(v: [u32; 5]) -> Self {
        Laser2Settings { pwm_freq_hz: v[0], min_power: v[1], max_power: v[2], standby_freq_hz: v[3], standby_pulse: v[4] }
    }
}

/// U axis and rotary settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UAxisSettings {
    pub control_flags: u32,
    pub step_length_um: u32,
    pub max_velocity: u32,
    pub start_velocity: u32,
    pub max_acc: u32,
    pub range_um: u32,
    pub home_offset_um: u32,
    pub backlash_um: u32,
    pub home_velocity: u32,
    pub work_velocity: u32,
    pub rotary_pulses_per_rotation: u32,
    pub rotary_diameter_um: u32,
}

impl UAxisSettings {
    pub const ADDRS: [u16; 12] = [
        addr::U_CONTROL,
        addr::U_PRECISION,
        addr::U_MAX_VELOCITY,
        addr::U_START_VELOCITY,
        addr::U_MAX_ACC,
        addr::U_RANGE,
        addr::U_HOME_OFFSET,
        addr::U_BACKLASH,
        addr::U_HOME_VELOCITY,
        addr::U_WORK_VELOCITY,
        addr::ROTATE_PULSE,
        addr::ROTATE_DIAMETER,
    ];

    pub fn to_values(self) -> [u32; 12] {
        [
            self.control_flags,
            self.step_length_um,
            self.max_velocity,
            self.start_velocity,
            self.max_acc,
            self.range_um,
            self.home_offset_um,
            self.backlash_um,
            self.home_velocity,
            self.work_velocity,
            self.rotary_pulses_per_rotation,
            self.rotary_diameter_um,
        ]
    }

    pub fn from_values(v: [u32; 12]) -> Self {
        UAxisSettings {
            control_flags: v[0],
            step_length_um: v[1],
            max_velocity: v[2],
            start_velocity: v[3],
            max_acc: v[4],
            range_um: v[5],
            home_offset_um: v[6],
            backlash_um: v[7],
            home_velocity: v[8],
            work_velocity: v[9],
            rotary_pulses_per_rotation: v[10],
            rotary_diameter_um: v[11],
        }
    }
}

/// Controller information (RUIDA-08).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ControllerInfo {
    pub card_id: u32,
    pub board: Option<String>,
    pub firmware: String,
    pub total_on_time_s: u32,
    pub total_job_time_s: u32,
    pub total_laser_on_time_s: u32,
    pub jobs_run: u32,
    pub last_job_time_ms: u32,
}

/// Decoded machine status word (address 0x200): a set of flags. Bit 0 = job running, bit 1 =
/// paused (MeerK40t calls it "part end"), bit 24 = moving. Paused is read as bits 0 and 1
/// together. MeerK40t's emulator answers the plain numbers 21, 22 and 23 instead; those are not
/// copied (the flags decode them as running, idle and paused by accident only).
/// UNCERTAIN until checked on a controller (HARDWARE-CHECKS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct StatusWord(pub u32);

impl StatusWord {
    pub const RUNNING: u32 = 0x0000_0001;
    pub const PART_END: u32 = 0x0000_0002;
    pub const MOVING: u32 = 0x0100_0000;
    /// What the emulator reports: no flags, running, running and paused.
    pub const IDLE_VALUE: u32 = 0;
    pub const RUNNING_VALUE: u32 = Self::RUNNING;
    pub const PAUSED_VALUE: u32 = Self::RUNNING | Self::PART_END;

    pub fn job_running(self) -> bool {
        self.0 & Self::RUNNING != 0 && !self.paused()
    }
    pub fn paused(self) -> bool {
        self.0 & (Self::RUNNING | Self::PART_END) == (Self::RUNNING | Self::PART_END)
    }
    pub fn moving(self) -> bool {
        self.0 & Self::MOVING != 0
    }
    pub fn state(self) -> MachineState {
        if self.paused() {
            MachineState::Paused
        } else if self.job_running() {
            MachineState::Running
        } else if self.moving() {
            MachineState::Moving
        } else {
            MachineState::Idle
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum MachineState {
    #[default]
    Idle,
    Moving,
    Running,
    Paused,
}

/// Status and position snapshot (positions in µm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct MachineStatus {
    pub word: StatusWord,
    pub state: MachineState,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub u: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_table_is_unique() {
        let mut addrs: Vec<u16> = REGISTERS.iter().map(|r| r.addr).collect();
        addrs.sort_unstable();
        let n = addrs.len();
        addrs.dedup();
        assert_eq!(n, addrs.len());
        assert!(REGISTERS.iter().all(|r| r.addr < 0x4000));
        assert_eq!(register(addr::BED_X).unwrap().name, "X travel (bed width)");
        assert!(writable(Some(Group::UAxis)).count() >= 8);
    }

    #[test]
    fn status_word_states() {
        assert_eq!(StatusWord(StatusWord::IDLE_VALUE).state(), MachineState::Idle);
        assert_eq!(StatusWord(StatusWord::RUNNING_VALUE).state(), MachineState::Running);
        assert_eq!(StatusWord(StatusWord::PAUSED_VALUE).state(), MachineState::Paused);
        assert_eq!(StatusWord(StatusWord::MOVING).state(), MachineState::Moving);
        assert_eq!(StatusWord(StatusWord::MOVING | StatusWord::RUNNING).state(), MachineState::Running, "a running job beats the moving flag");
        assert_eq!((StatusWord::IDLE_VALUE, StatusWord::RUNNING_VALUE, StatusWord::PAUSED_VALUE), (0, 1, 3), "flags, not MeerK40t's 22, 21, 23");
    }

    #[test]
    fn settings_backup_serde_and_diff() {
        let mut a = MachineSettings { card_id: Some(0x65106510), firmware: Some("X".into()), ..Default::default() };
        a.set(addr::BED_X, 600_000);
        a.set(addr::BED_Y, 400_000);
        assert_eq!(a.bed_size_mm(), Some((600.0, 400.0)));
        let json = serde_json::to_string(&a).unwrap();
        let b: MachineSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(a, b);
        let mut c = b.clone();
        c.set(addr::BED_Y, 1);
        c.set(addr::G0_VELOCITY, 5);
        assert_eq!(a.diff(&c), vec![addr::G0_VELOCITY, addr::BED_Y]);
    }

    #[test]
    fn struct_value_round_trip() {
        let l2 = Laser2Settings { pwm_freq_hz: 20000, min_power: 100, max_power: 900, standby_freq_hz: 1, standby_pulse: 2 };
        assert_eq!(Laser2Settings::from_values(l2.to_values()), l2);
        let u = UAxisSettings { range_um: 5, rotary_diameter_um: 80_000, ..Default::default() };
        assert_eq!(UAxisSettings::from_values(u.to_values()), u);
        assert_eq!(board_name(0x65106510), Some("RDC6442S"));
        assert_eq!(board_name(0x90109010), Some("RDC8445S"));
        assert_eq!(board_name(0x12345678), None);
        assert_eq!(board_name(0x65006500), Some("RDC6442G"));
        assert_eq!(board_label(0x90109010), "RDC8445S");
        assert_eq!(board_label(0x12345678), "Ruida controller (card ID 0x12345678, not in the board list)");
        // Every board of the checklist is listed; card IDs are unique.
        for part in ["644", "654", "658", "5121", "320", "420B", "6332G", "KT332N"] {
            assert!(KNOWN_BOARDS.iter().any(|b| b.model.contains(part)), "{part}");
        }
        let mut ids: Vec<u32> = KNOWN_BOARDS.iter().filter_map(|b| b.card_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }
}
