//! UDP packet framing for the Ruida data channel.
//!
//! A datagram to the controller is `[checksum hi][checksum lo][swizzled payload]` where the
//! checksum is the 16-bit sum of the **swizzled** payload bytes. The controller answers every
//! datagram with one swizzled byte: ACK (logical 0xCC, wire 0xC6 with magic 0x88), ERR
//! (logical 0xCD) or NAK (logical 0xCF, checksum mismatch). Replies to queries come in a
//! further datagram, swizzled, without a checksum prefix.
//!
//! USB carries the same swizzled stream with no checksum and no ACK byte.

use crate::error::{Result, RuidaError};

/// Controller data port.
pub const DATA_PORT: u16 = 50200;
/// Host source port the controller replies to.
pub const HOST_DATA_PORT: u16 = 40200;
/// Controller jog / wireless-panel port (unswizzled, no checksum).
pub const JOG_PORT: u16 = 50207;
/// Host source port for the jog channel.
pub const HOST_JOG_PORT: u16 = 40207;

/// Largest datagram the controller accepts, checksum included.
pub const MAX_DATAGRAM: usize = 1472;
/// Default payload per datagram. MeerK40t uses about 1000 bytes; well under the MTU.
pub const DEFAULT_CHUNK: usize = 1000;

/// 16-bit byte sum.
pub fn checksum(data: &[u8]) -> u16 {
    data.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32)) as u16
}

/// Build a datagram from an already swizzled payload.
pub fn frame(swizzled: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(swizzled.len() + 2);
    v.extend_from_slice(&checksum(swizzled).to_be_bytes());
    v.extend_from_slice(swizzled);
    v
}

/// Split a datagram into its payload, checking the checksum and size.
pub fn unframe(datagram: &[u8]) -> Result<&[u8]> {
    if datagram.len() < 2 {
        return Err(RuidaError::Decode("datagram shorter than its checksum".into()));
    }
    if datagram.len() > MAX_DATAGRAM {
        return Err(RuidaError::Decode(format!("datagram of {} bytes exceeds {MAX_DATAGRAM}", datagram.len())));
    }
    let want = u16::from_be_bytes([datagram[0], datagram[1]]);
    let payload = &datagram[2..];
    let got = checksum(payload);
    if want != got {
        return Err(RuidaError::Decode(format!("checksum 0x{want:04X} does not match payload sum 0x{got:04X}")));
    }
    Ok(payload)
}

/// Split `data` into chunks of at most `max` bytes (a chunk may end mid-command, which the
/// controller accepts). `max` is clamped to what fits in one datagram.
pub fn chunks(data: &[u8], max: usize) -> impl Iterator<Item = &[u8]> {
    let max = max.clamp(1, MAX_DATAGRAM - 2);
    data.chunks(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_wraps_at_16_bits() {
        assert_eq!(checksum(&[]), 0);
        assert_eq!(checksum(&[1, 2, 3]), 6);
        let big = vec![0xFFu8; 1000];
        assert_eq!(checksum(&big), (255u32 * 1000 % 65536) as u16);
    }

    #[test]
    fn frame_unframe_round_trip() {
        let payload = [0x12, 0x34, 0xFF, 0x00];
        let d = frame(&payload);
        assert_eq!(&d[..2], &[0x01, 0x45]);
        assert_eq!(unframe(&d).unwrap(), &payload);
    }

    #[test]
    fn bad_checksum_and_size_rejected() {
        let mut d = frame(&[1, 2, 3]);
        d[1] ^= 1;
        assert!(unframe(&d).is_err());
        assert!(unframe(&[0]).is_err());
        let huge = frame(&vec![0u8; MAX_DATAGRAM]);
        assert!(unframe(&huge).is_err());
    }

    #[test]
    fn chunks_respect_mtu() {
        let data = vec![7u8; 5000];
        let c: Vec<_> = chunks(&data, 10_000).collect();
        assert!(c.iter().all(|c| c.len() + 2 <= MAX_DATAGRAM));
        assert_eq!(c.iter().map(|c| c.len()).sum::<usize>(), 5000);
        assert_eq!(chunks(&data, 1000).count(), 5);
    }
}
