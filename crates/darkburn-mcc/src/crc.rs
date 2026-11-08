//! CRC-16/MODBUS as used by the MCC100 framing.
//!
//! Polynomial 0x8005 reflected (0xA001), init 0xFFFF, no final XOR, computed over frame bytes
//! `[4..]` (length field, unit, function, payload). The PC stores the value high byte first, the
//! card low byte first (standard RTU order). Four logged frames (CRC values 0x8C75, 0x4534,
//! 0x45D0, 0x85F5) match only this parameterisation (owner's analysis `04 section 3.2`, `08 section 2.2`).

/// Byte order of the CRC stored in frame bytes 2 and 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrcOrder {
    /// `[2] = crc >> 8`. Written by the PC.
    HiFirst,
    /// `[2] = crc & 0xFF`. Written by the card (every logged reply).
    LoFirst,
}

const fn build_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u16;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xA001 } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static TABLE: [u16; 256] = build_table();

/// CRC-16/MODBUS of `data`.
///
/// ```
/// assert_eq!(darkburn_mcc::crc16_modbus(&[0x00, 0x08, 0x00, 0x30, 0xe8, 0x03, 0x00, 0x00, 0x02, 0x00]), 0x8c75);
/// ```
pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc = (crc >> 8) ^ TABLE[((crc ^ b as u16) & 0xFF) as usize];
    }
    crc
}

/// The two stored CRC bytes for `crc` in the given order.
pub fn crc_bytes(crc: u16, order: CrcOrder) -> [u8; 2] {
    match order {
        CrcOrder::HiFirst => [(crc >> 8) as u8, crc as u8],
        CrcOrder::LoFirst => [crc as u8, (crc >> 8) as u8],
    }
}

/// Inverse of [`crc_bytes`].
pub fn crc_from_bytes(two: [u8; 2], order: CrcOrder) -> u16 {
    match order {
        CrcOrder::HiFirst => ((two[0] as u16) << 8) | two[1] as u16,
        CrcOrder::LoFirst => ((two[1] as u16) << 8) | two[0] as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four CRC values brute-forced from the logged frames (owner's `08 section 2.2`).
    #[test]
    fn logged_frames() {
        // PC request, hi first: 0000 8c75 0008 00 30 e8030000 0200
        assert_eq!(crc16_modbus(&[0x00, 0x08, 0x00, 0x30, 0xe8, 0x03, 0x00, 0x00, 0x02, 0x00]), 0x8c75);
        // Card exception replies, lo first.
        assert_eq!(crc16_modbus(&[0x00, 0x03, 0x00, 0xc0, 0x03]), 0x4534);
        assert_eq!(crc16_modbus(&[0x00, 0x03, 0x00, 0xb0, 0x02]), 0x45d0);
        assert_eq!(crc16_modbus(&[0x00, 0x03, 0x00, 0xc0, 0x02]), 0x85f5);
    }

    #[test]
    fn standard_check_value() {
        assert_eq!(crc16_modbus(b"123456789"), 0x4B37);
        assert_eq!(crc16_modbus(&[]), 0xFFFF);
    }

    #[test]
    fn order_round_trip() {
        for order in [CrcOrder::HiFirst, CrcOrder::LoFirst] {
            assert_eq!(crc_from_bytes(crc_bytes(0x8c75, order), order), 0x8c75);
        }
        assert_eq!(crc_bytes(0x8c75, CrcOrder::HiFirst), [0x8c, 0x75]);
        assert_eq!(crc_bytes(0x4534, CrcOrder::LoFirst), [0x34, 0x45]);
    }
}
