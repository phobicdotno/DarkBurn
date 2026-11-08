//! Plain-English tables for GRBL error codes, alarm codes and `$$` settings.
//!
//! Written from the GRBL v1.1 interface documentation (gnea/grbl wiki) and the grblHAL
//! "for sender developers" notes. These are interface facts (numbers and their meaning);
//! the wording here is DarkBurn's own.

use crate::protocol::FirmwareKind;

/// A plain-English description of a controller error (`error:N`).
pub fn error_message(code: u16) -> String {
    let text = match code {
        1 => "A G-code word has no letter in front of it.",
        2 => "A number in the command is missing or is not a valid number.",
        3 => "The controller does not know this '$' command.",
        4 => "A negative value was given where only a positive value is allowed.",
        5 => "Homing is turned off in the controller settings ($22).",
        6 => "The step pulse time must be at least 3 microseconds ($0).",
        7 => "The controller could not read its settings memory and has restored the defaults.",
        8 => "This '$' command can only run while the machine is idle.",
        9 => "G-code is locked while the machine is in alarm or jogging. Home or unlock first.",
        10 => "Soft limits need homing to be turned on ($22) before they can be enabled.",
        11 => "The line is too long for the controller and was not run.",
        12 => "The step rate is too high for the controller with these settings.",
        13 => "The safety door is open.",
        14 => "The build info or startup line is too long to store.",
        15 => "The jog would move past the machine's travel limits.",
        16 => "The jog command is not valid (only axis words, F and G20/G21/G90/G91/G53 are allowed).",
        17 => "Laser mode needs a PWM output on the controller.",
        20 => "The controller does not support this G-code or M-code.",
        21 => "Two commands from the same group are on one line.",
        22 => "No feed rate (F) has been set for this move.",
        23 => "This G-code needs a whole number value.",
        24 => "Two commands on this line both need axis words.",
        25 => "A word is repeated on the same line.",
        26 => "The command needs axis words but none were given.",
        27 => "The line number (N) is not valid.",
        28 => "The command is missing a required P or L value.",
        29 => "The controller does not support work coordinate systems G59.1 to G59.3.",
        30 => "G53 can only be used with G0 or G1.",
        31 => "There are axis words on the line that nothing uses.",
        32 => "The arc has no axis words in the selected plane.",
        33 => "The move target is not valid.",
        34 => "The arc radius or geometry cannot be computed.",
        35 => "The arc is missing its I, J or K offset in the selected plane.",
        36 => "There are words on the line that nothing uses.",
        37 => "Tool length offset (G43.1) was given for the wrong axis.",
        38 => "The tool number is larger than the controller allows.",
        // grblHAL extensions
        39 => "A value is out of range.",
        40 => "A tool change is waiting to be confirmed.",
        41 => "The spindle or laser is not running.",
        42 => "The selected plane is not allowed for this command.",
        43 => "The feed rate is higher than the machine allows.",
        44 => "The spindle speed or laser power is out of range.",
        45 => "A limit switch is engaged.",
        46 => "The machine must be homed first.",
        47 => "Tool error.",
        48 => "Two values conflict on this line.",
        49 => "The controller's self test failed.",
        50 => "The emergency stop is engaged.",
        51 => "A motor driver reported a fault.",
        52 => "The setting value is out of range.",
        53 => "This setting is turned off.",
        54 => "The retract position is not valid.",
        55 => "The homing configuration is not valid.",
        56 => "The coordinate system is locked.",
        58 => "The port is not available.",
        59..=64 => "The controller could not evaluate an expression on this line.",
        _ => return format!("Controller error {code}."),
    };
    text.to_owned()
}

/// A plain-English description of a controller alarm (`ALARM:N`).
///
/// Alarm 10 differs between firmwares: grblHAL uses it for the emergency stop, GRBL 1.1h for a
/// failed dual-axis homing.
pub fn alarm_message(code: u16, firmware: FirmwareKind) -> String {
    let text = match code {
        1 => "Hard limit: a limit switch was hit while moving. Position is likely lost; home the machine.",
        2 => "Soft limit: the move goes outside the machine's travel. Position is kept; unlock to continue.",
        3 => "Reset while moving: position may be lost. Home the machine before continuing.",
        4 => "Probe fail: the probe was already triggered before probing started.",
        5 => "Probe fail: the probe did not touch anything within the search distance.",
        6 => "Homing fail: homing was reset or stopped.",
        7 => "Homing fail: the safety door was opened during homing.",
        8 => "Homing fail: pulling off the switch did not clear it. Check wiring or pull-off distance ($27).",
        9 => "Homing fail: a limit switch was not found. Check wiring, max travel ($130-$132) or homing direction ($23).",
        10 if firmware == FirmwareKind::GrblHal => "Emergency stop is engaged.",
        10 => "Homing fail: the second switch of a dual-motor axis was not found.",
        11 => "Homing is required before the machine can move.",
        12 => "A limit switch was engaged when the controller started.",
        13 => "Probe protection was triggered.",
        14 => "The spindle did not reach speed in time.",
        15 => "Homing fail: the axis could not be squared.",
        16 => "The controller's power-on self test failed.",
        17 => "A motor driver reported a fault.",
        18 => "Homing fail: the homing configuration is not valid.",
        19 => "The Modbus connection failed.",
        20 => "The I/O expander failed.",
        21 => "The controller could not read its settings memory.",
        22 => "A buffer overflowed in the controller.",
        _ => return format!("Controller alarm {code}."),
    };
    text.to_owned()
}

/// True for alarms after which GRBL waits for a reset before it accepts anything else.
pub fn alarm_is_critical(code: u16) -> bool {
    matches!(code, 1 | 2 | 10)
}

/// What a `$` setting means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingInfo {
    pub number: u16,
    /// Short label for a settings table.
    pub label: &'static str,
    /// Unit shown next to the value ("" when the value has no unit).
    pub unit: &'static str,
    /// One-sentence explanation.
    pub description: &'static str,
    /// The value is a bit mask (X = 1, Y = 2, Z = 4).
    pub is_mask: bool,
    /// The value is on/off (0 or 1).
    pub is_bool: bool,
}

const fn s(number: u16, label: &'static str, unit: &'static str, description: &'static str) -> SettingInfo {
    SettingInfo { number, label, unit, description, is_mask: false, is_bool: false }
}
const fn m(number: u16, label: &'static str, description: &'static str) -> SettingInfo {
    SettingInfo { number, label, unit: "mask", description, is_mask: true, is_bool: false }
}
const fn b(number: u16, label: &'static str, description: &'static str) -> SettingInfo {
    SettingInfo { number, label, unit: "", description, is_mask: false, is_bool: true }
}

/// Every documented GRBL 1.1 setting plus the common grblHAL laser settings.
pub const SETTINGS: &[SettingInfo] = &[
    s(0, "Step pulse time", "µs", "How long each step pulse lasts."),
    s(1, "Step idle delay", "ms", "How long motors stay powered after a move (255 keeps them on)."),
    m(2, "Step pulse invert", "Inverts the step signal for the axes in the mask."),
    m(3, "Step direction invert", "Reverses the direction of the axes in the mask."),
    b(4, "Invert step enable", "Inverts the stepper enable pin."),
    b(5, "Invert limit pins", "Inverts the limit switch inputs."),
    b(6, "Invert probe pin", "Inverts the probe input."),
    m(10, "Status report options", "What the status report contains (1 = machine position, 2 = buffer state)."),
    s(11, "Junction deviation", "mm", "How fast the machine takes corners; smaller is slower and smoother."),
    s(12, "Arc tolerance", "mm", "How closely arcs are split into straight segments."),
    b(13, "Report in inches", "Positions are reported in inches instead of millimetres."),
    b(20, "Soft limits", "Rejects moves outside the machine travel (needs homing)."),
    b(21, "Hard limits", "Stops the machine when a limit switch is hit."),
    b(22, "Homing cycle", "Turns on homing with $H."),
    m(23, "Homing direction invert", "Homes the axes in the mask towards the negative end."),
    s(24, "Homing feed", "mm/min", "Slow speed used to find the switch exactly."),
    s(25, "Homing seek", "mm/min", "Fast speed used to search for the switch."),
    s(26, "Homing debounce", "ms", "Wait time for the switch signal to settle."),
    s(27, "Homing pull-off", "mm", "How far to back away from the switch after homing."),
    s(30, "Maximum S value", "S", "The S value that gives full laser power (or maximum spindle speed)."),
    s(31, "Minimum S value", "S", "The S value that gives the lowest power."),
    b(32, "Laser mode", "Keeps moving between power changes and turns the laser off on rapid moves."),
    s(33, "PWM frequency", "Hz", "Laser PWM frequency (grblHAL)."),
    s(34, "PWM off value", "%", "PWM duty when the laser is off (grblHAL)."),
    s(35, "PWM minimum value", "%", "PWM duty at the lowest power (grblHAL)."),
    s(36, "PWM maximum value", "%", "PWM duty at full power (grblHAL)."),
    s(100, "X steps per mm", "steps/mm", "Motor steps needed to move the X axis one millimetre."),
    s(101, "Y steps per mm", "steps/mm", "Motor steps needed to move the Y axis one millimetre."),
    s(102, "Z steps per mm", "steps/mm", "Motor steps needed to move the Z axis one millimetre."),
    s(110, "X max rate", "mm/min", "Fastest X speed, also used for rapid moves."),
    s(111, "Y max rate", "mm/min", "Fastest Y speed, also used for rapid moves."),
    s(112, "Z max rate", "mm/min", "Fastest Z speed, also used for rapid moves."),
    s(120, "X acceleration", "mm/s²", "How quickly X speeds up and slows down."),
    s(121, "Y acceleration", "mm/s²", "How quickly Y speeds up and slows down."),
    s(122, "Z acceleration", "mm/s²", "How quickly Z speeds up and slows down."),
    s(130, "X max travel", "mm", "Length of the X axis, used by soft limits."),
    s(131, "Y max travel", "mm", "Length of the Y axis, used by soft limits."),
    s(132, "Z max travel", "mm", "Length of the Z axis, used by soft limits."),
];

/// Looks up the label and description of a `$` setting.
pub fn setting_info(number: u16) -> Option<&'static SettingInfo> {
    SETTINGS.iter().find(|i| i.number == number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grbl_error_has_a_message() {
        for code in (1..=38).filter(|c| !(18..=19).contains(c)) {
            let msg = error_message(code);
            assert!(!msg.starts_with("Controller error"), "error {code} has no text");
        }
        assert_eq!(error_message(999), "Controller error 999.");
    }

    #[test]
    fn alarms_depend_on_firmware() {
        assert!(alarm_message(1, FirmwareKind::Grbl).starts_with("Hard limit"));
        assert!(alarm_message(10, FirmwareKind::GrblHal).contains("Emergency"));
        assert!(alarm_message(10, FirmwareKind::Grbl).contains("second switch"));
        for code in 1..=9 {
            assert!(!alarm_message(code, FirmwareKind::Grbl).starts_with("Controller alarm"));
        }
    }

    #[test]
    fn settings_have_labels() {
        for n in [0, 1, 2, 3, 4, 5, 6, 10, 11, 12, 13, 20, 21, 22, 23, 24, 25, 26, 27, 30, 31, 32] {
            assert!(setting_info(n).is_some(), "${n}");
        }
        for n in 100..=102 {
            assert!(setting_info(n).is_some());
            assert!(setting_info(n + 10).is_some());
            assert!(setting_info(n + 20).is_some());
            assert!(setting_info(n + 30).is_some());
        }
        assert_eq!(setting_info(32).unwrap().label, "Laser mode");
        assert!(setting_info(999).is_none());
    }
}
