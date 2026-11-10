//! Ruida number encodings. Every parameter byte is 7-bit (< 0x80), so a command byte (>= 0x80)
//! always marks the start of the next command.
//!
//! * int7: one byte.
//! * int14: two 7-bit groups, high first. Unsigned for power and file numbers, 14-bit two's
//!   complement for relative coordinates (-8192..=8191 µm).
//! * int35: five 7-bit groups (shifts 28, 21, 14, 7, 0). Absolute coordinates, speeds, times,
//!   frequencies, colours and settings values. Decoded as the low 32 bits, so negative i32
//!   values round-trip.
//!
//! Units on the wire: coordinates in µm, speed in µm/s, time in µs, frequency in Hz,
//! power as a 14-bit value on a scale where 0x4000 is 100 % (percent = raw / 163.84, so the
//! largest value that fits, 0x3FFF, reads 99.994 %), colour as 0x00BBGGRR. The power scale
//! follows ruida-pa and MeerK40t (both MIT, see ATTRIBUTION.md); no controller has confirmed it.

use crate::error::{Result, RuidaError};

/// Largest relative move that fits int14.
pub const REL_MIN: i32 = -8192;
pub const REL_MAX: i32 = 8191;
/// Largest power value that fits 14 bits (sent for 100 %).
pub const POWER_FULL: u16 = 0x3FFF;
/// Raw power units per percent: 0x4000 / 100.
pub const POWER_PER_PERCENT: f64 = 163.84;

pub fn encode_u14(v: u16) -> [u8; 2] {
    [((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}

pub fn decode_u14(b: &[u8]) -> Result<u16> {
    need(b, 2)?;
    Ok(((b[0] as u16 & 0x7F) << 7) | (b[1] as u16 & 0x7F))
}

/// Encode a signed relative coordinate. Values outside -8192..=8191 are an error.
pub fn encode_i14(v: i32) -> Result<[u8; 2]> {
    if !(REL_MIN..=REL_MAX).contains(&v) {
        return Err(RuidaError::Encode(format!("relative value {v} does not fit 14 bits")));
    }
    Ok(encode_u14((v as u16) & 0x3FFF))
}

pub fn decode_i14(b: &[u8]) -> Result<i32> {
    let u = decode_u14(b)? as i32;
    Ok(if u >= 0x2000 { u - 0x4000 } else { u })
}

pub fn encode_u35(v: u32) -> [u8; 5] {
    [((v >> 28) & 0x7F) as u8, ((v >> 21) & 0x7F) as u8, ((v >> 14) & 0x7F) as u8, ((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
}

pub fn decode_u35(b: &[u8]) -> Result<u32> {
    need(b, 5)?;
    let mut v: u64 = 0;
    for &x in &b[..5] {
        v = (v << 7) | (x as u64 & 0x7F);
    }
    Ok(v as u32)
}

pub fn encode_i35(v: i32) -> [u8; 5] {
    let w = v as u32;
    // Sign-extend into the top group so negative numbers look like the controller's.
    let top = if v < 0 { ((w >> 28) | 0x70) & 0x7F } else { (w >> 28) & 0x7F };
    let mut out = encode_u35(w);
    out[0] = top as u8;
    out
}

pub fn decode_i35(b: &[u8]) -> Result<i32> {
    Ok(decode_u35(b)? as i32)
}

fn need(b: &[u8], n: usize) -> Result<()> {
    if b.len() < n { Err(RuidaError::Decode(format!("need {n} bytes, have {}", b.len()))) } else { Ok(()) }
}

/// Millimetres to µm, rounded.
pub fn mm_to_um(mm: f64) -> i32 {
    (mm * 1000.0).round() as i32
}

pub fn um_to_mm(um: i32) -> f64 {
    um as f64 / 1000.0
}

/// mm/s to µm/s, rounded.
pub fn mm_s_to_um_s(mm_s: f64) -> u32 {
    (mm_s * 1000.0).round().max(0.0) as u32
}

pub fn um_s_to_mm_s(um_s: u32) -> f64 {
    um_s as f64 / 1000.0
}

/// Milliseconds to µs, rounded.
pub fn ms_to_us(ms: f64) -> u32 {
    (ms * 1000.0).round().max(0.0) as u32
}

pub fn us_to_ms(us: u32) -> f64 {
    us as f64 / 1000.0
}

/// A laser power value as the controller stores it (0..=16383).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct Power(pub u16);

impl Power {
    pub const FULL: Power = Power(POWER_FULL);

    /// From a percentage, clamped to 0..=100.
    pub fn from_percent(pct: f64) -> Power {
        Power(((pct.clamp(0.0, 100.0) * POWER_PER_PERCENT).round() as u16).min(POWER_FULL))
    }

    /// Percent as the controller counts it: raw / 163.84. Full power reads 99.994.
    pub fn percent(self) -> f64 {
        self.0.min(POWER_FULL) as f64 / POWER_PER_PERCENT
    }

    pub fn encode(self) -> [u8; 2] {
        encode_u14(self.0.min(POWER_FULL))
    }

    pub fn decode(b: &[u8]) -> Result<Power> {
        Ok(Power(decode_u14(b)?))
    }
}

/// Convert an RGB colour (0xRRGGBB) to Ruida's 0x00BBGGRR and back.
pub fn rgb_to_bgr(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    (b << 16) | (g << 8) | r
}

pub fn bgr_to_rgb(bgr: u32) -> u32 {
    rgb_to_bgr(bgr)
}

/// Encode a NUL-terminated 7-bit string. Bytes >= 0x80 are replaced by `_`.
pub fn encode_cstring(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().map(|b| if b >= 0x80 || b == 0 { b'_' } else { b }).collect();
    v.push(0);
    v
}

/// Decode a NUL-terminated string; a missing terminator is accepted (end of slice).
/// Returns the string and the number of bytes used, including the terminator.
pub fn decode_cstring(b: &[u8]) -> (String, usize) {
    match b.iter().position(|&x| x == 0) {
        Some(i) => (String::from_utf8_lossy(&b[..i]).into_owned(), i + 1),
        None => (String::from_utf8_lossy(b).into_owned(), b.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u14_round_trip() {
        for v in [0u16, 1, 127, 128, 0x1FFF, 0x2000, 0x3FFF] {
            let e = encode_u14(v);
            assert!(e.iter().all(|&b| b < 0x80));
            assert_eq!(decode_u14(&e).unwrap(), v);
        }
    }

    #[test]
    fn i14_round_trip_and_range() {
        for v in [REL_MIN, -1, 0, 1, 100, -100, REL_MAX] {
            let e = encode_i14(v).unwrap();
            assert!(e.iter().all(|&b| b < 0x80));
            assert_eq!(decode_i14(&e).unwrap(), v);
        }
        assert_eq!(encode_i14(-1).unwrap(), [0x7F, 0x7F]);
        assert!(encode_i14(8192).is_err());
        assert!(encode_i14(-8193).is_err());
    }

    #[test]
    fn i35_round_trip() {
        for v in [0, 1, -1, 1000, -1000, 600_000, i32::MAX, i32::MIN, 123_456_789] {
            let e = encode_i35(v);
            assert!(e.iter().all(|&b| b < 0x80));
            assert_eq!(decode_i35(&e).unwrap(), v);
        }
        assert_eq!(encode_i35(-1), [0x7F; 5]);
        for v in [0u32, 1, 0x65106510, u32::MAX] {
            assert_eq!(decode_u35(&encode_u35(v)).unwrap(), v);
        }
    }

    #[test]
    fn power_conversions() {
        assert_eq!(Power::from_percent(100.0), Power::FULL);
        assert_eq!(Power::from_percent(0.0), Power(0));
        assert_eq!(Power::from_percent(150.0), Power::FULL);
        assert!((Power::from_percent(50.0).percent() - 50.0).abs() < 0.01);
        // 14 bits on a scale where 0x4000 is 100 %: raw / 163.84, about 0.006 % per step.
        assert_eq!(Power::from_percent(50.0), Power(8192));
        assert_eq!(Power::from_percent(25.0), Power(4096));
        assert_eq!(Power::from_percent(0.1), Power(16));
        assert_eq!(Power(8192).percent(), 50.0);
        assert!((Power::FULL.percent() - 99.994).abs() < 0.001);
        assert!((Power(1).percent() - 0.0061).abs() < 0.0001);
        assert_eq!(Power::decode(&Power(1234).encode()).unwrap(), Power(1234));
    }

    #[test]
    fn units_and_colour() {
        assert_eq!(mm_to_um(12.3456), 12346);
        assert_eq!(um_to_mm(-1500), -1.5);
        assert_eq!(mm_s_to_um_s(100.0), 100_000);
        assert_eq!(ms_to_us(1.5), 1500);
        assert_eq!(rgb_to_bgr(0x112233), 0x332211);
        assert_eq!(bgr_to_rgb(rgb_to_bgr(0xABCDEF)), 0xABCDEF);
    }

    #[test]
    fn cstrings() {
        assert_eq!(encode_cstring("JOB1"), b"JOB1\0");
        assert_eq!(decode_cstring(b"JOB1\0rest"), ("JOB1".to_string(), 5));
        assert_eq!(decode_cstring(b"AB"), ("AB".to_string(), 2));
    }

    #[test]
    fn short_input_is_error() {
        assert!(decode_u14(&[1]).is_err());
        assert!(decode_u35(&[1, 2, 3]).is_err());
    }
}
