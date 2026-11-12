//! The receiver's two reports, as observed on the device. Pure functions, no I/O.
//!
//! * Input report 4, 6 bytes with the id: `04 10 k1 k2 k3 chk`. `k1..k3` are up to three keys
//!   held at the same time (0 for none). The report is the whole set of keys held now; all zero
//!   means nothing is held. Byte 1 and `chk` are not keys and are not checked (`chk` changes its
//!   base after LED writes).
//! * Feature report 6, 8 bytes with the id: `06 fe fd SEQ LEDS 00 00 00`. `SEQ` is any byte and
//!   goes up by one per message. `LEDS` is a bit mask, see [`Leds`].

use std::fmt;

/// USB vendor id of the receiver (Chengdu XHC).
pub const VENDOR_ID: u16 = 0x10ce;

/// The receiver ids DarkBurn opens. XHC makes about twenty receivers in the range
/// `0xeb61..=0xeba1` that may speak the same reports, but only `0xeba0` (the orange MLASER
/// pendant, model PHB02) has been checked on a real device. Add others here only after they are
/// checked.
pub const PRODUCT_IDS: &[u16] = &[0xeba0];

/// The vendor-defined HID usage page the receiver reports on.
pub const USAGE_PAGE: u16 = 0xff00;

/// Report id of the key report.
pub const INPUT_REPORT_ID: u8 = 0x04;
/// Length of the key report, report id included.
pub const INPUT_REPORT_LEN: usize = 6;
/// Report id of the LED report.
pub const LED_REPORT_ID: u8 = 0x06;
/// Length of the LED report, report id included.
pub const LED_REPORT_LEN: usize = 8;

/// A key on the pendant, by the code it sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Key {
    Follow = 0x01,
    Shutter = 0x02,
    RedLight = 0x03,
    Laser = 0x04,
    Blow = 0x05,
    Frame = 0x06,
    GoToOrigin = 0x07,
    Start = 0x08,
    Pause = 0x09,
    Stop = 0x0a,
    /// An ordinary key: Fn+X arrives as the two keys held together.
    Fn = 0x0b,
    ZCalibrate = 0x0c,
    FastStep = 0x0d,
    ZMinusW = 0x0e,
    Up = 0x0f,
    ZPlusW = 0x10,
    Left = 0x11,
    Down = 0x12,
    Right = 0x13,
}

impl Key {
    /// Every key, in code order.
    pub const ALL: [Key; 19] = [
        Key::Follow,
        Key::Shutter,
        Key::RedLight,
        Key::Laser,
        Key::Blow,
        Key::Frame,
        Key::GoToOrigin,
        Key::Start,
        Key::Pause,
        Key::Stop,
        Key::Fn,
        Key::ZCalibrate,
        Key::FastStep,
        Key::ZMinusW,
        Key::Up,
        Key::ZPlusW,
        Key::Left,
        Key::Down,
        Key::Right,
    ];

    /// The key a code stands for; `None` for 0 (no key) and unknown codes.
    pub fn from_code(code: u8) -> Option<Key> {
        Key::ALL.iter().copied().find(|k| *k as u8 == code)
    }

    /// The code the pendant sends for this key.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The keys with a second function printed for Fn: Fn+GoToOrigin, Fn+ZCalibrate,
    /// Fn+ZMinusW and Fn+ZPlusW. Fn with any other key is still reported as a chord, except
    /// Stop and Pause, which are always plain presses.
    pub fn has_printed_fn(self) -> bool {
        matches!(self, Key::GoToOrigin | Key::ZCalibrate | Key::ZMinusW | Key::ZPlusW)
    }

    /// A short name for logs and messages.
    pub fn name(self) -> &'static str {
        match self {
            Key::Follow => "Follow",
            Key::Shutter => "Shutter",
            Key::RedLight => "Red light",
            Key::Laser => "Laser",
            Key::Blow => "Blow",
            Key::Frame => "Frame",
            Key::GoToOrigin => "Go to origin",
            Key::Start => "Start",
            Key::Pause => "Pause",
            Key::Stop => "Stop",
            Key::Fn => "Fn",
            Key::ZCalibrate => "Z calibrate",
            Key::FastStep => "Fast/step",
            Key::ZMinusW => "Z-/W",
            Key::Up => "Up",
            Key::ZPlusW => "Z+/W",
            Key::Left => "Left",
            Key::Down => "Down",
            Key::Right => "Right",
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The set of keys held at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeySet(u32);

impl KeySet {
    pub const EMPTY: KeySet = KeySet(0);

    pub fn insert(&mut self, k: Key) {
        self.0 |= 1 << k.code();
    }
    pub fn remove(&mut self, k: Key) {
        self.0 &= !(1 << k.code());
    }
    pub fn contains(self, k: Key) -> bool {
        self.0 & (1 << k.code()) != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }
    /// The keys in code order.
    pub fn iter(self) -> impl Iterator<Item = Key> {
        Key::ALL.into_iter().filter(move |k| self.contains(*k))
    }
    /// Keys in `self` that are not in `other`.
    pub fn minus(self, other: KeySet) -> KeySet {
        KeySet(self.0 & !other.0)
    }
}

impl FromIterator<Key> for KeySet {
    fn from_iter<I: IntoIterator<Item = Key>>(iter: I) -> Self {
        let mut s = KeySet::EMPTY;
        for k in iter {
            s.insert(k);
        }
        s
    }
}

/// Why a report was not a key report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer bytes than a key report has.
    TooShort(usize),
    /// Another report id.
    OtherReport(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooShort(n) => write!(f, "pendant report too short ({n} bytes)"),
            DecodeError::OtherReport(id) => write!(f, "not a key report (report id {id:#04x})"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A decoded key report: the keys held and the codes in it that DarkBurn does not know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyReport {
    pub held: KeySet,
    pub unknown: Vec<u8>,
}

/// Decodes input report 4 (`04 10 k1 k2 k3 chk`, report id first). Byte 1 and the last byte are
/// ignored; a report cut after `k3` is accepted.
pub fn decode_input(report: &[u8]) -> Result<KeyReport, DecodeError> {
    let Some(&id) = report.first() else { return Err(DecodeError::TooShort(0)) };
    if id != INPUT_REPORT_ID {
        return Err(DecodeError::OtherReport(id));
    }
    if report.len() < INPUT_REPORT_LEN - 1 {
        return Err(DecodeError::TooShort(report.len()));
    }
    let mut out = KeyReport::default();
    for &code in &report[2..5] {
        match (code, Key::from_code(code)) {
            (0, _) => {}
            (_, Some(k)) => out.held.insert(k),
            (c, None) => out.unknown.push(c),
        }
    }
    Ok(out)
}

/// The LEDs the host may set. Signal and battery LEDs are run by the pendant itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Leds(pub u8);

impl Leds {
    pub const NONE: Leds = Leds(0);
    pub const LASER: Leds = Leds(0x01);
    pub const BLOW: Leds = Leds(0x02);
    pub const FRAME: Leds = Leds(0x04);
    pub const FOLLOW: Leds = Leds(0x10);
    pub const SHUTTER: Leds = Leds(0x20);
    pub const RED_LIGHT: Leds = Leds(0x40);
    /// Bits with an LED behind them (0x08 and 0x80 are unused).
    pub const MASK: u8 = 0x01 | 0x02 | 0x04 | 0x10 | 0x20 | 0x40;

    pub fn contains(self, other: Leds) -> bool {
        self.0 & other.0 == other.0
    }
    /// `self` with `other` switched on or off.
    pub fn with(self, other: Leds, on: bool) -> Leds {
        if on { Leds(self.0 | other.0) } else { Leds(self.0 & !other.0) }
    }
    /// The LED that sits on a key, if it has one.
    pub fn of_key(k: Key) -> Option<Leds> {
        Some(match k {
            Key::Laser => Leds::LASER,
            Key::Blow => Leds::BLOW,
            Key::Frame => Leds::FRAME,
            Key::Follow => Leds::FOLLOW,
            Key::Shutter => Leds::SHUTTER,
            Key::RedLight => Leds::RED_LIGHT,
            _ => return None,
        })
    }
}

impl std::ops::BitOr for Leds {
    type Output = Leds;
    fn bitor(self, rhs: Leds) -> Leds {
        Leds(self.0 | rhs.0)
    }
}

/// Encodes feature report 6 for `leds`, with sequence byte `seq`. Unused bits are cleared.
pub fn encode_leds(seq: u8, leds: Leds) -> [u8; LED_REPORT_LEN] {
    [LED_REPORT_ID, 0xfe, 0xfd, seq, leds.0 & Leds::MASK, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(r: &[u8]) -> Vec<Key> {
        decode_input(r).unwrap().held.iter().collect()
    }

    #[test]
    fn every_key_code_decodes_to_its_key() {
        let table: [(u8, Key); 19] = [
            (0x01, Key::Follow),
            (0x02, Key::Shutter),
            (0x03, Key::RedLight),
            (0x04, Key::Laser),
            (0x05, Key::Blow),
            (0x06, Key::Frame),
            (0x07, Key::GoToOrigin),
            (0x08, Key::Start),
            (0x09, Key::Pause),
            (0x0a, Key::Stop),
            (0x0b, Key::Fn),
            (0x0c, Key::ZCalibrate),
            (0x0d, Key::FastStep),
            (0x0e, Key::ZMinusW),
            (0x0f, Key::Up),
            (0x10, Key::ZPlusW),
            (0x11, Key::Left),
            (0x12, Key::Down),
            (0x13, Key::Right),
        ];
        for (code, key) in table {
            assert_eq!(Key::from_code(code), Some(key));
            assert_eq!(key.code(), code);
            // chk = 0x10 - k1, as first seen on the device.
            let r = [0x04, 0x10, code, 0x00, 0x00, 0x10u8.wrapping_sub(code)];
            assert_eq!(held(&r), vec![key], "code {code:#04x}");
        }
        assert_eq!(Key::from_code(0), None);
        assert_eq!(Key::from_code(0x14), None);
        assert_eq!(Key::from_code(0xff), None);
    }

    #[test]
    fn idle_report_holds_nothing() {
        let r = decode_input(&[0x04, 0x10, 0x00, 0x00, 0x00, 0x10]).unwrap();
        assert!(r.held.is_empty());
        assert!(r.unknown.is_empty());
        assert!(decode_input(&[0x04, 0x00, 0x00, 0x00, 0x00, 0x00]).unwrap().held.is_empty());
    }

    #[test]
    fn up_to_three_keys_are_held_at_once() {
        assert_eq!(held(&[0x04, 0x10, 0x0b, 0x07, 0x00, 0x05]), vec![Key::GoToOrigin, Key::Fn]);
        assert_eq!(held(&[0x04, 0x10, 0x0f, 0x11, 0x0d, 0x00]), vec![Key::FastStep, Key::Up, Key::Left]);
        // The order inside the report does not matter.
        assert_eq!(held(&[0x04, 0x10, 0x07, 0x0b, 0x00, 0x00]), held(&[0x04, 0x10, 0x0b, 0x07, 0x00, 0x00]));
    }

    #[test]
    fn checksum_and_byte_one_are_not_checked() {
        // The checksum base moves after LED writes: any value is accepted.
        for chk in [0x00, 0x0f, 0x7f, 0xff] {
            assert_eq!(held(&[0x04, 0x10, 0x08, 0x00, 0x00, chk]), vec![Key::Start]);
        }
        assert_eq!(held(&[0x04, 0x22, 0x0a, 0x00, 0x00, 0x00]), vec![Key::Stop]);
        // A report cut after k3 still reads.
        assert_eq!(held(&[0x04, 0x10, 0x09, 0x00, 0x00]), vec![Key::Pause]);
    }

    #[test]
    fn unknown_codes_are_kept_apart() {
        let r = decode_input(&[0x04, 0x10, 0x08, 0x2a, 0x00, 0x00]).unwrap();
        assert_eq!(r.held.iter().collect::<Vec<_>>(), vec![Key::Start]);
        assert_eq!(r.unknown, vec![0x2a]);
    }

    #[test]
    fn other_reports_and_short_reads_are_refused() {
        assert_eq!(decode_input(&[]), Err(DecodeError::TooShort(0)));
        assert_eq!(decode_input(&[0x04, 0x10, 0x08]), Err(DecodeError::TooShort(3)));
        assert_eq!(decode_input(&[0x06, 0xfe, 0xfd, 0x01, 0x01, 0x00, 0x00]), Err(DecodeError::OtherReport(0x06)));
        assert_eq!(decode_input(&[0x00, 0x10, 0x08, 0x00, 0x00, 0x00]), Err(DecodeError::OtherReport(0x00)));
    }

    #[test]
    fn led_report_bytes() {
        assert_eq!(encode_leds(0x00, Leds::NONE), [0x06, 0xfe, 0xfd, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x01, Leds::LASER), [0x06, 0xfe, 0xfd, 0x01, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x02, Leds::BLOW), [0x06, 0xfe, 0xfd, 0x02, 0x02, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x03, Leds::FRAME), [0x06, 0xfe, 0xfd, 0x03, 0x04, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x04, Leds::FOLLOW), [0x06, 0xfe, 0xfd, 0x04, 0x10, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x05, Leds::SHUTTER), [0x06, 0xfe, 0xfd, 0x05, 0x20, 0x00, 0x00, 0x00]);
        assert_eq!(encode_leds(0x06, Leds::RED_LIGHT), [0x06, 0xfe, 0xfd, 0x06, 0x40, 0x00, 0x00, 0x00]);
        let all = Leds::LASER | Leds::BLOW | Leds::FRAME | Leds::FOLLOW | Leds::SHUTTER | Leds::RED_LIGHT;
        assert_eq!(encode_leds(0xff, all), [0x06, 0xfe, 0xfd, 0xff, 0x77, 0x00, 0x00, 0x00]);
        // The unused bits 0x08 and 0x80 never go out.
        assert_eq!(encode_leds(0x07, Leds(0xff))[4], 0x77);
        assert_eq!(encode_leds(0, Leds::NONE).len(), LED_REPORT_LEN);
    }

    #[test]
    fn leds_sit_on_their_keys() {
        assert_eq!(Leds::of_key(Key::Laser), Some(Leds::LASER));
        assert_eq!(Leds::of_key(Key::Blow), Some(Leds::BLOW));
        assert_eq!(Leds::of_key(Key::Frame), Some(Leds::FRAME));
        assert_eq!(Leds::of_key(Key::Follow), Some(Leds::FOLLOW));
        assert_eq!(Leds::of_key(Key::Shutter), Some(Leds::SHUTTER));
        assert_eq!(Leds::of_key(Key::RedLight), Some(Leds::RED_LIGHT));
        assert_eq!(Leds::of_key(Key::Start), None);
        let l = Leds::NONE.with(Leds::BLOW, true).with(Leds::FRAME, true).with(Leds::BLOW, false);
        assert_eq!(l, Leds::FRAME);
        assert!((Leds::FRAME | Leds::LASER).contains(Leds::LASER));
    }

    #[test]
    fn key_set_basics() {
        let mut s: KeySet = [Key::Fn, Key::Up].into_iter().collect();
        assert_eq!(s.len(), 2);
        assert!(s.contains(Key::Fn) && !s.contains(Key::Down));
        s.remove(Key::Fn);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![Key::Up]);
        let t: KeySet = [Key::Up, Key::Down].into_iter().collect();
        assert_eq!(t.minus(s).iter().collect::<Vec<_>>(), vec![Key::Down]);
        assert!(KeySet::EMPTY.is_empty());
    }

    #[test]
    fn only_the_checked_receiver_is_enabled() {
        assert_eq!(VENDOR_ID, 0x10ce);
        assert_eq!(PRODUCT_IDS, &[0xeba0]);
        assert!(Key::GoToOrigin.has_printed_fn() && Key::ZPlusW.has_printed_fn() && !Key::Start.has_printed_fn());
    }
}
