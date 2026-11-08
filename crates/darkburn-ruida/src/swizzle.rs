//! Byte scrambling ("swizzle") used by Ruida controllers on every channel except the jog port.
//!
//! Per byte: swap bit 0 and bit 7, XOR with the controller's magic value, add one (mod 256).
//! Unswizzle is the exact inverse. The same transform applies to `.rd` files, UDP and USB.
//! The swizzle maths is a protocol fact documented by MeerK40t (MIT) and EduTechWiki.

/// A controller's swizzle magic value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Magic(pub u8);

impl Magic {
    /// RDC644x, RDC6442G/S, RDC6445, RDLC320/230, 633X/654XG.
    pub const DEFAULT: Magic = Magic(0x88);
    /// 634XG.
    pub const RDC634XG: Magic = Magic(0x11);
    /// RDL9635 (RDCAM v3).
    pub const RDL9635: Magic = Magic(0x38);

    /// The magic values tried first when probing an unknown controller.
    pub const COMMON: [Magic; 3] = [Magic::DEFAULT, Magic::RDC634XG, Magic::RDL9635];
}

impl Default for Magic {
    fn default() -> Self {
        Magic::DEFAULT
    }
}

#[inline]
fn swap_bit0_bit7(b: u8) -> u8 {
    (b & 0x7E) | ((b & 0x01) << 7) | ((b & 0x80) >> 7)
}

/// Swizzle one logical byte.
#[inline]
pub fn swizzle_byte(b: u8, magic: Magic) -> u8 {
    (swap_bit0_bit7(b) ^ magic.0).wrapping_add(1)
}

/// Unswizzle one wire byte.
#[inline]
pub fn unswizzle_byte(b: u8, magic: Magic) -> u8 {
    swap_bit0_bit7(b.wrapping_sub(1) ^ magic.0)
}

/// Precomputed lookup tables for one magic value.
#[derive(Clone)]
pub struct Swizzler {
    magic: Magic,
    enc: [u8; 256],
    dec: [u8; 256],
}

impl std::fmt::Debug for Swizzler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Swizzler").field("magic", &self.magic).finish()
    }
}

impl Swizzler {
    pub fn new(magic: Magic) -> Self {
        let mut enc = [0u8; 256];
        let mut dec = [0u8; 256];
        for i in 0..=255u8 {
            enc[i as usize] = swizzle_byte(i, magic);
            dec[i as usize] = unswizzle_byte(i, magic);
        }
        Swizzler { magic, enc, dec }
    }

    pub fn magic(&self) -> Magic {
        self.magic
    }

    pub fn swizzle(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.enc[b as usize]).collect()
    }

    pub fn unswizzle(&self, data: &[u8]) -> Vec<u8> {
        data.iter().map(|&b| self.dec[b as usize]).collect()
    }

    pub fn swizzle_in_place(&self, data: &mut [u8]) {
        for b in data {
            *b = self.enc[*b as usize];
        }
    }

    pub fn unswizzle_in_place(&self, data: &mut [u8]) {
        for b in data {
            *b = self.dec[*b as usize];
        }
    }
}

/// Swizzle a buffer with `magic`.
pub fn swizzle(data: &[u8], magic: Magic) -> Vec<u8> {
    data.iter().map(|&b| swizzle_byte(b, magic)).collect()
}

/// Unswizzle a buffer with `magic`.
pub fn unswizzle(data: &[u8], magic: Magic) -> Vec<u8> {
    data.iter().map(|&b| unswizzle_byte(b, magic)).collect()
}

/// Guess the magic of a swizzled `.rd` file from its byte histogram.
///
/// Logical 0x00 is by far the most common byte in job files and swizzles to `magic + 1`,
/// so the most frequent wire byte minus one is the likely magic. Runs of the same byte count
/// extra, as MeerK40t's detector does. Returns `None` for empty input.
pub fn detect_magic_histogram(data: &[u8]) -> Option<Magic> {
    if data.is_empty() {
        return None;
    }
    let mut hist = [0u64; 256];
    let mut prev: Option<u8> = None;
    for &d in data {
        hist[d as usize] += if prev == Some(d) { 5 } else { 1 };
        prev = Some(d);
    }
    let (best, _) = hist.iter().enumerate().max_by_key(|&(i, v)| (*v, std::cmp::Reverse(i)))?;
    Some(Magic((best as u8).wrapping_sub(1)))
}

/// Check that a swizzled buffer decodes to a plausible command stream under `magic`:
/// it must start with a command byte (>= 0x80). Used together with the histogram guess.
pub fn plausible_stream(data: &[u8], magic: Magic) -> bool {
    data.first().is_some_and(|&b| unswizzle_byte(b, magic) >= 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_and_err_wire_bytes_for_default_magic() {
        // Logical ACK 0xCC is 0xC6 on the wire, ERR 0xCD is 0x46 (documented facts).
        assert_eq!(swizzle_byte(0xCC, Magic::DEFAULT), 0xC6);
        assert_eq!(swizzle_byte(0xCD, Magic::DEFAULT), 0x46);
        assert_eq!(swizzle_byte(0x00, Magic::DEFAULT), 0x89);
    }

    #[test]
    fn round_trip_all_bytes_for_known_and_arbitrary_magics() {
        let mut magics = vec![Magic(0x88), Magic(0x11), Magic(0x38)];
        magics.extend((0..=255u8).step_by(7).map(Magic));
        for magic in magics {
            let s = Swizzler::new(magic);
            let mut seen = [false; 256];
            for b in 0..=255u8 {
                let w = swizzle_byte(b, magic);
                assert!(!seen[w as usize], "swizzle must be a bijection");
                seen[w as usize] = true;
                assert_eq!(unswizzle_byte(w, magic), b);
                assert_eq!(s.swizzle(&[b])[0], w);
                assert_eq!(s.unswizzle(&[w])[0], b);
            }
        }
    }

    #[test]
    fn buffer_helpers_match() {
        let data: Vec<u8> = (0..=255).collect();
        for magic in Magic::COMMON {
            let s = Swizzler::new(magic);
            let w = swizzle(&data, magic);
            assert_eq!(w, s.swizzle(&data));
            assert_eq!(unswizzle(&w, magic), data);
            let mut buf = data.clone();
            s.swizzle_in_place(&mut buf);
            s.unswizzle_in_place(&mut buf);
            assert_eq!(buf, data);
        }
    }

    #[test]
    fn histogram_detects_magic() {
        let mut logical = vec![0xD8, 0x12, 0x88];
        logical.extend(std::iter::repeat_n(0u8, 200));
        logical.extend([0xA8, 0x01, 0x02, 0x03, 0x04, 0x05, 0xD7]);
        for magic in [Magic(0x88), Magic(0x11), Magic(0x38), Magic(0x5A)] {
            let wire = swizzle(&logical, magic);
            assert_eq!(detect_magic_histogram(&wire), Some(magic));
            assert!(plausible_stream(&wire, magic));
        }
        assert_eq!(detect_magic_histogram(&[]), None);
    }
}
