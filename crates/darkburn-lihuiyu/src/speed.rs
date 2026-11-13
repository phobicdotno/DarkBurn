//! Speed codes: how a Lihuiyu board is told how fast to move.
//!
//! A speed code holds a 16-bit timer value (the delay between stepper pulses), an acceleration
//! digit (1 to 4) and, for most boards, a step value and a diagonal correction. The timer
//! value follows `value = 65536 - (m * T + b)` where `T` is the time per mil in milliseconds
//! (`25.4 / speed`) and `b`, `m` depend on the board, the acceleration and the "suffix C" form.
//!
//! Forms written here:
//!
//! * vector, boards with step codes: `CV{value}{accel}{step}{diag}` or, in suffix-C form,
//!   `CV{value}1{step}{diag}C`;
//! * vector, boards A, B and M: `CV{value}{accel}` or `CV{value}1C`;
//! * raster: `V{value}{accel}G{step}` (bidirectional) or `V{value}{accel}G{g0}G{g1}`
//!   (unidirectional).
//!
//! Values are written as two three-digit numbers (high byte, low byte). A negative high byte
//! is written the way the original software wrote it, as an eight-digit number.
//!
//! The equations and every special case are ported from MeerK40t's `laserspeed.py` (MIT); see
//! `ATTRIBUTION.md`.

use std::fmt;

/// Lihuiyu board models. They differ in their speed code equations and forms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Board {
    /// M2 Nano, the most common stock K40 board.
    #[default]
    M2,
    /// M3 Nano. Speed codes as the M2.
    M3,
    /// B2: half-stepping, twice the M2 slope.
    B2,
    /// M: no step or diagonal values in vector codes.
    M,
    M1,
    /// A: no step or diagonal values in vector codes.
    A,
    /// B: no step or diagonal values in vector codes.
    B,
    B1,
}

impl Board {
    pub const ALL: [Board; 8] = [Board::M2, Board::M3, Board::B2, Board::M, Board::M1, Board::A, Board::B, Board::B1];

    /// The board's name as printed on it ("M2", "B1", ...).
    pub fn name(self) -> &'static str {
        match self {
            Board::M2 => "M2",
            Board::M3 => "M3",
            Board::B2 => "B2",
            Board::M => "M",
            Board::M1 => "M1",
            Board::A => "A",
            Board::B => "B",
            Board::B1 => "B1",
        }
    }

    /// The board named `name` (case and surrounding spaces ignored).
    pub fn from_name(name: &str) -> Option<Board> {
        let n = name.trim();
        Board::ALL.into_iter().find(|b| b.name().eq_ignore_ascii_case(n))
    }

    /// True when vector speed codes carry a step value and a diagonal correction.
    pub fn has_step_code(self) -> bool {
        !matches!(self, Board::A | Board::B | Board::M)
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The raster step of a raster speed code, in mils.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterStep {
    /// Steps on every reversal (both directions burn).
    Bidirectional(u32),
    /// Two values used on alternate reversals (`(step, 0)` burns one direction only).
    Unidirectional(u32, u32),
}

/// The diagonal correction ratio the original software used.
pub const DEFAULT_D_RATIO: f64 = 0.261199033289;

/// Measured ratio of real to requested speed on an M2 board (about 92 %).
pub const FIX_SPEEDS_RATIO: f64 = 0.919493599053179;

/// Options for [`speed_code`]. `Default` gives the plain encoder (M2, automatic acceleration
/// and suffix C, no limits fixed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedOptions {
    pub board: Board,
    /// `Some` makes a raster code. A bidirectional step of 0 counts as no raster step.
    pub raster_step: Option<RasterStep>,
    /// Diagonal ratio; `None` is [`DEFAULT_D_RATIO`]. 0 leaves the step and diagonal out.
    pub d_ratio: Option<f64>,
    /// Forces the acceleration digit (1..=4).
    pub acceleration: Option<u8>,
    /// Forces the suffix-C form on or off; `None` picks it from the board and speed.
    pub suffix_c: Option<bool>,
    /// Off: vector speeds above 240 mm/s become 19.05 mm/s, as the original software did.
    pub fix_limit: bool,
    /// Uses the measured M2 slope so the real speed matches the requested one.
    pub fix_speeds: bool,
    /// Clamps values that would be negative (speeds below the board's slowest) to 0.
    pub fix_lows: bool,
    /// A horizontal raster moves the light head, not the gantry; it allows other accelerations.
    pub raster_horizontal: bool,
}

impl Default for SpeedOptions {
    fn default() -> Self {
        SpeedOptions {
            board: Board::M2,
            raster_step: None,
            d_ratio: None,
            acceleration: None,
            suffix_c: None,
            fix_limit: false,
            fix_speeds: false,
            fix_lows: false,
            raster_horizontal: true,
        }
    }
}

impl SpeedOptions {
    /// Options for `board` with everything else at its default.
    pub fn for_board(board: Board) -> Self {
        SpeedOptions { board, ..Default::default() }
    }
}

/// `(b, m)` of `value = 65536 - (m * T + b)` for a board, acceleration and form.
pub fn equation(board: Board, accel: u8, suffix_c: bool, fix_speeds: bool) -> (f64, f64) {
    let mut b = match accel {
        3 => 896.0,
        4 => 1024.0,
        _ => 784.0,
    };
    if matches!(board, Board::A | Board::B | Board::B1) {
        // No suffix-C equations are known for these boards.
        return (b, 2000.0);
    }
    let mut m = if fix_speeds { 11148.0 } else { 12120.0 };
    if board == Board::B2 {
        m *= 2.0;
        if suffix_c {
            return (b, m / 12.0);
        }
    } else {
        b = match accel {
            3 => 5632.0,
            4 => 6144.0,
            _ => 5120.0,
        };
        if suffix_c {
            return (8.0, m / 12.0);
        }
    }
    (b, m)
}

/// The acceleration digit the original software picked for a speed.
pub fn acceleration_for_speed(speed: f64, raster: bool, raster_horizontal: bool, fix_speeds: bool) -> u8 {
    let speed = if fix_speeds { speed / FIX_SPEEDS_RATIO } else { speed };
    if speed <= 25.4 {
        1
    } else if speed <= 60.0 {
        2
    } else if raster && raster_horizontal {
        if speed < 127.0 {
            2
        } else if speed <= 320.0 {
            3
        } else {
            4
        }
    } else if speed < 127.0 {
        3
    } else {
        4
    }
}

/// Whether the original software used the suffix-C form for this board and speed.
pub fn suffix_c_for(board: Board, speed: f64) -> bool {
    matches!(board, Board::B2 | Board::M2 | Board::M3) && speed < 7.0
}

/// The timer value for `speed` mm/s (before any clamping).
pub fn value_from_speed(speed: f64, b: f64, m: f64) -> f64 {
    if speed == 0.0 {
        return 65536.0 - b;
    }
    let frequency_khz = speed / 25.4;
    let period_ms = 1.0 / frequency_khz;
    65536.0 - (m * period_ms + b)
}

/// The speed in mm/s for a timer value (`value` already turned back from `65536 - x`).
pub fn speed_from_value(value: f64, b: f64, m: f64) -> f64 {
    if m == 0.0 {
        return 0.0;
    }
    let period_ms = (value - b) / m;
    if period_ms == 0.0 {
        return 0.0;
    }
    25.4 * (1.0 / period_ms)
}

/// Writes a 16-bit value as high and low byte, three digits each. The fraction is dropped
/// (towards zero); a negative value gives the eight-digit high byte the original software wrote.
pub fn encode_16bit(value: f64) -> String {
    let v = value as i64;
    let b0 = v & 255;
    let b1 = (v >> 8) & 0xFF_FFFF;
    format!("{b1:03}{b0:03}")
}

/// Reads a value written by [`encode_16bit`] (also the eight-digit form). `None` for text that
/// is not at least four digits.
pub fn decode_16bit(code: &str) -> Option<i64> {
    if code.len() < 4 || !code.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (hi, lo) = code.split_at(code.len() - 3);
    let mut b1: i64 = hi.parse().ok()?;
    if b1 > 16_000_000 {
        b1 -= 16_777_216;
    }
    if b1 > 0x7FFF {
        // Kept as the original decoder has it.
        b1 -= 0xFFFF;
    }
    let b2: i64 = lo.parse().ok()?;
    Some(b1 * 256 + b2)
}

fn raster_of(o: &SpeedOptions) -> Option<RasterStep> {
    o.raster_step.filter(|r| *r != RasterStep::Bidirectional(0))
}

/// The speed code for `speed` mm/s.
pub fn speed_code(speed: f64, o: &SpeedOptions) -> String {
    let d_ratio = o.d_ratio.unwrap_or(DEFAULT_D_RATIO);
    let raster = raster_of(o);
    let mut speed = speed;
    if !o.fix_limit && speed > 240.0 && raster.is_none() {
        speed = 19.05;
    }
    let accel = o.acceleration.unwrap_or_else(|| acceleration_for_speed(speed, raster.is_some(), o.raster_horizontal, o.fix_speeds));
    let suffix_c = o.suffix_c.unwrap_or_else(|| suffix_c_for(o.board, speed));
    let (b, m) = equation(o.board, accel, suffix_c, o.fix_speeds);
    let mut value = value_from_speed(speed, b, m);
    if o.fix_lows && value < 0.0 {
        value = 0.0;
    }
    let encoded = encode_16bit(value);
    match raster {
        Some(RasterStep::Bidirectional(g)) => return format!("V{encoded}{accel}G{g:03}"),
        Some(RasterStep::Unidirectional(g0, g1)) => return format!("V{encoded}{accel}G{g0:03}G{g1:03}"),
        None => {}
    }
    if d_ratio == 0.0 || !o.board.has_step_code() {
        return if suffix_c { format!("CV{encoded}1C") } else { format!("CV{encoded}{accel}") };
    }
    let step_value = ((speed.floor() as i64) + 1).min(128);
    let frequency_khz = speed / 25.4;
    let period_ms = if frequency_khz == 0.0 { 0.0 } else { 1.0 / frequency_khz };
    let mut d_value = d_ratio * m * period_ms / step_value as f64;
    if o.fix_lows {
        d_value = d_value.clamp(0.0, 65535.0);
    }
    let diagonal = encode_16bit(d_value);
    if suffix_c { format!("CV{encoded}1{step_value:03}{diagonal}C") } else { format!("CV{encoded}{accel}{step_value:03}{diagonal}") }
}

/// The parts of a speed code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedSpeedCode {
    /// The timer value, already turned back (`65536 - written value`).
    pub value: i64,
    pub acceleration: u8,
    /// Step value (0 when the code has none).
    pub step: u32,
    /// Diagonal correction (0 when the code has none).
    pub diagonal: i64,
    pub raster_step: Option<RasterStep>,
    pub prefix_c: bool,
    pub suffix_c: bool,
}

/// Why a speed code could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeedCodeError(pub String);

impl fmt::Display for SpeedCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a speed code: {}", self.0)
    }
}

impl std::error::Error for SpeedCodeError {}

fn digits(code: &str, from: usize, to: usize) -> Option<&str> {
    let s = code.get(from..to)?;
    (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit())).then_some(s)
}

/// Splits a speed code into its parts.
pub fn parse_speed_code(code: &str) -> Result<ParsedSpeedCode, SpeedCodeError> {
    let err = || SpeedCodeError(code.chars().take(40).collect());
    let s = code.as_bytes();
    if !code.is_ascii() || s.is_empty() {
        return Err(err());
    }
    let mut start = 0;
    let mut end = s.len();
    let mut prefix_c = false;
    let mut suffix_c = false;
    if s[0] == b'C' {
        start += 1;
        prefix_c = true;
    }
    if end > start && s[end - 1] == b'C' {
        end -= 1;
        suffix_c = true;
    }
    if s.get(start) != Some(&b'V') {
        return Err(err());
    }
    let bugged = s.get(start..start + 4) == Some(b"V167".as_slice()) && s.get(start + 4).is_some_and(|c| !matches!(c, b'0' | b'1' | b'2'));
    let width = if bugged { 11 } else { 6 };
    let written = digits(code, start + 1, start + 1 + width).and_then(decode_16bit).ok_or_else(err)?;
    start += 1 + width;
    let value = 65536 - written;
    let acceleration = digits(code, start, start + 1).and_then(|d| d.parse::<u8>().ok()).ok_or_else(err)?;
    start += 1;
    if end < start {
        return Err(err());
    }
    let mut raster_step = None;
    if end >= start + 4 && s[end - 4] == b'G' {
        let g: u32 = digits(code, end - 3, end).and_then(|d| d.parse().ok()).ok_or_else(err)?;
        raster_step = Some(RasterStep::Bidirectional(g));
        end -= 4;
        if end >= start + 4 && s[end - 4] == b'G' {
            let g0: u32 = digits(code, end - 3, end).and_then(|d| d.parse().ok()).ok_or_else(err)?;
            raster_step = Some(RasterStep::Unidirectional(g0, g));
            end -= 4;
        }
    }
    let mut step = 0;
    let mut diagonal = 0;
    if end + 1 >= start + 9 {
        step = digits(code, start, start + 3).and_then(|d| d.parse().ok()).ok_or_else(err)?;
        diagonal = digits(code, start + 3, end).and_then(decode_16bit).ok_or_else(err)?;
    } else if end != start {
        return Err(err());
    }
    Ok(ParsedSpeedCode { value, acceleration, step, diagonal, raster_step, prefix_c, suffix_c })
}

/// The speed in mm/s a speed code asks for on `board`.
pub fn speed_from_code(code: &str, board: Board, fix_speeds: bool) -> Result<f64, SpeedCodeError> {
    let p = parse_speed_code(code)?;
    let (b, m) = equation(board, p.acceleration, p.suffix_c, fix_speeds);
    Ok(speed_from_value(p.value as f64, b, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver_vector(board: Board) -> SpeedOptions {
        SpeedOptions { board, suffix_c: Some(true), fix_limit: true, fix_lows: true, ..Default::default() }
    }

    #[test]
    fn m2_vector_code_matches_the_reference_driver() {
        // The rectangle job of MeerK40t's driver tests, M2 at 15 mm/s.
        assert_eq!(speed_code(15.0, &driver_vector(Board::M2)), "CV2490731016000027C");
    }

    #[test]
    fn m2_raster_code_matches_the_reference_driver() {
        let o = SpeedOptions {
            raster_step: Some(RasterStep::Bidirectional(2)),
            fix_limit: true,
            fix_lows: true,
            suffix_c: Some(false),
            ..Default::default()
        };
        assert_eq!(speed_code(15.0, &o), "V1552121G002");
    }

    #[test]
    fn unidirectional_raster_has_two_g_values() {
        let o = SpeedOptions { raster_step: Some(RasterStep::Unidirectional(3, 0)), ..Default::default() };
        let c = speed_code(100.0, &o);
        assert!(c.starts_with('V') && c.ends_with("G003G000"), "{c}");
        let p = parse_speed_code(&c).unwrap();
        assert_eq!(p.raster_step, Some(RasterStep::Unidirectional(3, 0)));
    }

    #[test]
    fn boards_without_step_codes_use_the_short_form() {
        for board in [Board::A, Board::B, Board::M] {
            let c = speed_code(30.0, &SpeedOptions::for_board(board));
            assert_eq!(c.len(), 9, "{board}: {c}");
            let c = speed_code(30.0, &driver_vector(board));
            assert!(c.ends_with("1C") && c.len() == 10, "{board}: {c}");
        }
        let c = speed_code(30.0, &SpeedOptions { d_ratio: Some(0.0), ..Default::default() });
        assert_eq!(c.len(), 9, "{c}");
    }

    #[test]
    fn acceleration_steps_with_speed() {
        assert_eq!(acceleration_for_speed(10.0, false, true, false), 1);
        assert_eq!(acceleration_for_speed(25.4, false, true, false), 1);
        assert_eq!(acceleration_for_speed(40.0, false, true, false), 2);
        assert_eq!(acceleration_for_speed(100.0, false, true, false), 3);
        assert_eq!(acceleration_for_speed(100.0, true, true, false), 2);
        assert_eq!(acceleration_for_speed(100.0, true, false, false), 3);
        assert_eq!(acceleration_for_speed(200.0, true, true, false), 3);
        assert_eq!(acceleration_for_speed(320.0, true, true, false), 3);
        assert_eq!(acceleration_for_speed(400.0, true, true, false), 4);
        assert_eq!(acceleration_for_speed(200.0, false, true, false), 4);
        // Corrected speeds count as faster.
        assert_eq!(acceleration_for_speed(24.0, false, true, true), 2);
    }

    #[test]
    fn suffix_c_only_for_slow_m2_m3_b2() {
        assert!(suffix_c_for(Board::M2, 6.9));
        assert!(suffix_c_for(Board::M3, 1.0));
        assert!(suffix_c_for(Board::B2, 5.0));
        assert!(!suffix_c_for(Board::M2, 7.0));
        assert!(!suffix_c_for(Board::M1, 1.0));
        assert!(!suffix_c_for(Board::A, 1.0));
    }

    #[test]
    fn fast_vector_speeds_fall_back_unless_the_limit_is_fixed() {
        let plain = SpeedOptions::default();
        assert_eq!(speed_code(300.0, &plain), speed_code(19.05, &plain));
        let fixed = SpeedOptions { fix_limit: true, ..Default::default() };
        assert_ne!(speed_code(300.0, &fixed), speed_code(19.05, &fixed));
    }

    #[test]
    fn low_speeds_clamp_when_asked() {
        let o = SpeedOptions { fix_lows: true, ..Default::default() };
        let c = speed_code(0.05, &o);
        assert!(c.starts_with("CV000000"), "{c}");
        let unclamped = speed_code(0.05, &SpeedOptions::default());
        assert!(!unclamped.starts_with("CV000000"), "{unclamped}");
    }

    #[test]
    fn sixteen_bit_round_trip() {
        for v in [0.0, 1.0, 255.0, 256.0, 48296.0, 65535.0, 63817.7] {
            assert_eq!(decode_16bit(&encode_16bit(v)), Some(v as i64));
        }
        assert_eq!(encode_16bit(36176.0), "141080");
        assert_eq!(decode_16bit("12"), None);
        assert_eq!(decode_16bit("12a456"), None);
    }

    #[test]
    fn negative_values_use_the_eight_digit_form() {
        let s = encode_16bit(-1000.0);
        assert_eq!(s.len(), 11, "{s}");
        assert_eq!(decode_16bit(&s), Some(-1000));
    }

    #[test]
    fn parse_reads_every_part() {
        let p = parse_speed_code("CV2490731016000027C").unwrap();
        assert_eq!(p.value, 65536 - (249 * 256 + 73));
        assert_eq!(p.acceleration, 1);
        assert_eq!(p.step, 16);
        assert_eq!(p.diagonal, 27);
        assert!(p.prefix_c && p.suffix_c);
        assert_eq!(p.raster_step, None);
        let p = parse_speed_code("V1552121G002").unwrap();
        assert_eq!(p.raster_step, Some(RasterStep::Bidirectional(2)));
        assert!(!p.prefix_c && !p.suffix_c);
    }

    #[test]
    fn parse_rejects_garbage_without_panicking() {
        for bad in ["", "C", "CC", "X123", "CV", "CV12", "CV123456", "CVabcdef1", "CV1234561G0x2", "V167", "CV1234561234", "\u{e9}V"] {
            assert!(parse_speed_code(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn speed_round_trips_on_every_board_and_gear() {
        for board in Board::ALL {
            for i in (1..2400).step_by(3) {
                let speed = i as f64 / 10.0;
                let c = speed_code(speed, &SpeedOptions::for_board(board));
                let back = speed_from_code(&c, board, false).unwrap();
                assert!((back - speed).abs() <= speed / 200.0, "{board} {speed} {c} {back}");
                for accel in 1..=4 {
                    let o = SpeedOptions { acceleration: Some(accel), suffix_c: Some(false), ..SpeedOptions::for_board(board) };
                    let c = speed_code(speed, &o);
                    let back = speed_from_code(&c, board, false).unwrap();
                    assert!((back - speed).abs() <= speed / 200.0, "{board} {speed} gear {accel} {c} {back}");
                }
            }
        }
    }

    #[test]
    fn suffix_c_gear_one_round_trips() {
        for board in [Board::B2, Board::M, Board::M1, Board::M2, Board::M3] {
            for i in (1..2400).step_by(3) {
                let speed = i as f64 / 10.0;
                let o = SpeedOptions { acceleration: Some(1), suffix_c: Some(true), ..SpeedOptions::for_board(board) };
                let c = speed_code(speed, &o);
                let back = speed_from_code(&c, board, false).unwrap();
                assert!((back - speed).abs() <= speed / 100.0, "{board} {speed} {c} {back}");
            }
        }
    }

    #[test]
    fn raster_speeds_round_trip() {
        for board in Board::ALL {
            for i in (140..5000).step_by(3) {
                let speed = i as f64 / 10.0;
                for step in [RasterStep::Bidirectional(2), RasterStep::Unidirectional(2, 1)] {
                    let o = SpeedOptions { raster_step: Some(step), ..SpeedOptions::for_board(board) };
                    let c = speed_code(speed, &o);
                    let back = speed_from_code(&c, board, false).unwrap();
                    assert!((back - speed).abs() <= speed / 100.0, "{board} {speed} {c} {back}");
                }
            }
        }
    }

    #[test]
    fn fixed_speeds_round_trip_with_the_same_setting() {
        for i in (10..2400).step_by(7) {
            let speed = i as f64 / 10.0;
            let o = SpeedOptions { fix_speeds: true, fix_limit: true, ..Default::default() };
            let c = speed_code(speed, &o);
            let back = speed_from_code(&c, Board::M2, true).unwrap();
            assert!((back - speed).abs() <= speed / 200.0, "{speed} {c} {back}");
        }
    }

    #[test]
    fn every_code_has_a_valid_length() {
        for board in Board::ALL {
            for i in (0..25000).step_by(7) {
                let speed = i as f64 / 100.0;
                let o = SpeedOptions { fix_lows: true, ..SpeedOptions::for_board(board) };
                let c = speed_code(speed, &o);
                let c = c.strip_suffix('C').unwrap_or(&c);
                assert!(c.len() == 18 || c.len() == 9, "{board} {speed} {c}");
            }
        }
    }

    #[test]
    fn board_names() {
        for b in Board::ALL {
            assert_eq!(Board::from_name(b.name()), Some(b));
            assert_eq!(Board::from_name(&format!(" {} ", b.name().to_lowercase())), Some(b));
        }
        assert_eq!(Board::from_name("M9"), None);
        assert_eq!(Board::default(), Board::M2);
        assert_eq!(Board::B1.to_string(), "B1");
    }
}
