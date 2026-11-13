//! CH341 packet framing for Lihuiyu boards.
//!
//! EGV text is sent in packets of 30 payload bytes. A packet ends early at a line end (`\n`);
//! a short packet is padded with `F` (which the board reads as "nothing more"). Each packet is
//! sent as `0x00`, the 30 bytes and a CRC-8 (Dallas / Maxim one-wire CRC of the 30 bytes). The
//! CH341 in EPP mode puts its write command byte `0xA6` in front of every 32 bytes, so on the
//! wire a packet is 33 bytes: `A6 00 <30 bytes> <crc>`.
//!
//! A few characters just before a line end are not sent; they tell the sender what to do after
//! the packet ("meta" characters):
//!
//! | char | meaning |
//! |---|---|
//! | `-` | wait until the board reports it has finished |
//! | `*` | abort |
//! | `!` | pause |
//! | `&` | resume |
//! | `%` | send with an inverted CRC (the board will reject it) |
//! | `#` | pad with the packet's last byte instead of `F` |
//! | `\x18` | stop the sender |
//!
//! A packet that ends in `P` gets one extra `F` (some M3 boards miss the end otherwise), and a
//! lone line end becomes a packet of 30 `F`.
//!
//! After each packet the sender reads the status (6 bytes, the second one matters) until the
//! board says OK; ERROR means the CRC did not match and the packet is sent again.
//!
//! Ported from MeerK40t's `lihuiyu/controller.py` and `ch341/libusb.py` (MIT); see
//! `ATTRIBUTION.md`. Nothing here talks to a device.

/// Payload bytes per packet.
pub const PAYLOAD_LEN: usize = 30;
/// Bytes of one packet as handed to the CH341: `0x00`, payload, CRC.
pub const PACKET_LEN: usize = 32;
/// The CH341 EPP write command byte put in front of every 32 bytes.
pub const CH341_EPP_WRITE: u8 = 0xA6;
/// Bytes of one packet on the USB wire (command byte included).
pub const WIRE_LEN: usize = 33;

/// Dallas / Maxim one-wire CRC-8 (reflected polynomial 0x8C, start 0).
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8C } else { crc >> 1 };
        }
    }
    crc
}

/// What the sender does after a packet (from its meta character).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// Wait until the board reports finished (`-`).
    WaitFinish,
    /// Abort the job (`*`).
    Abort,
    /// Pause sending (`!`).
    Pause,
    /// Resume sending (`&`).
    Resume,
    /// Stop the sender (`\x18`).
    Quit,
    /// The packet is a serial number challenge (it starts with `A`); confirm the answer.
    ConfirmSerial,
}

/// One framed packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    pub payload: [u8; PAYLOAD_LEN],
    /// Sent with an inverted CRC so the board rejects it (`%`).
    pub bad_crc: bool,
}

impl Packet {
    /// The CRC byte as sent.
    pub fn crc(&self) -> u8 {
        let c = crc8(&self.payload);
        if self.bad_crc { c ^ 0xFF } else { c }
    }

    /// `0x00`, payload, CRC.
    pub fn to_bytes(&self) -> [u8; PACKET_LEN] {
        let mut out = [0u8; PACKET_LEN];
        out[1..31].copy_from_slice(&self.payload);
        out[31] = self.crc();
        out
    }

    /// The 33 bytes on the USB wire.
    pub fn to_wire(&self) -> [u8; WIRE_LEN] {
        let mut out = [0u8; WIRE_LEN];
        out[0] = CH341_EPP_WRITE;
        out[1..].copy_from_slice(&self.to_bytes());
        out
    }
}

/// One step of the send queue: a packet to send (or none, when the step only carried a meta
/// character) and what to do after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub packet: Option<Packet>,
    pub after: Option<After>,
}

/// The result of [`frames`]: the frames, and how many bytes at the end were left because they
/// do not make a packet yet (fewer than 30 and no line end).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Framed {
    pub frames: Vec<Frame>,
    pub unsent: usize,
}

fn is_meta(c: u8) -> bool {
    matches!(c, b'-' | b'*' | b'&' | b'!' | b'#' | b'%' | 0x18)
}

/// Splits EGV bytes into packets the way the original sender does.
///
/// Differs from it in one corner: a meta character that is the 30th byte of a packet and is
/// not followed by a line end is sent as an ordinary byte (the original sender would stall).
pub fn frames(data: &[u8]) -> Framed {
    let mut out = Framed::default();
    let mut rest = data;
    while !rest.is_empty() {
        let window = &rest[..rest.len().min(PAYLOAD_LEN)];
        let mut length = match window.iter().position(|&c| c == b'\n') {
            Some(i) => i + 1,
            None => window.len(),
        };
        // A meta character right before a line end that fell just outside the window.
        if length == PAYLOAD_LEN && is_meta(rest[length - 1]) && rest.get(length) == Some(&b'\n') {
            length += 1;
        }
        let raw = &rest[..length];
        if raw.last() != Some(&b'\n') && length < PAYLOAD_LEN {
            out.unsent = rest.len();
            return out;
        }
        rest = &rest[length..];
        let at = raw.starts_with(b"AT");
        let mut body: Vec<u8> = raw.to_vec();
        let mut after = None;
        let mut bad_crc = false;
        let mut pad = if at { 0u8 } else { b'F' };
        if body.last() == Some(&b'\n') {
            body.pop();
            if body.is_empty() {
                body.push(b'F');
            }
            match body.last().copied() {
                Some(b'P') => body.push(b'F'),
                Some(b'-') => {
                    body.pop();
                    after = Some(After::WaitFinish);
                }
                Some(b'*') => {
                    body.pop();
                    after = Some(After::Abort);
                }
                Some(b'&') => {
                    body.pop();
                    after = Some(After::Resume);
                }
                Some(b'!') => {
                    body.pop();
                    after = Some(After::Pause);
                }
                Some(b'%') => {
                    body.pop();
                    bad_crc = true;
                }
                Some(0x18) => {
                    body.pop();
                    after = Some(After::Quit);
                }
                _ => {}
            }
            if body.first() == Some(&b'A') && !at {
                after = Some(After::ConfirmSerial);
            }
            if body.last() == Some(&b'#') {
                body.pop();
                pad = body.last().copied().unwrap_or(b'F');
            }
        }
        let packet = if body.is_empty() {
            None
        } else {
            body.truncate(PAYLOAD_LEN);
            body.resize(PAYLOAD_LEN, pad);
            let mut payload = [0u8; PAYLOAD_LEN];
            payload.copy_from_slice(&body);
            Some(Packet { payload, bad_crc })
        };
        out.frames.push(Frame { packet, after });
    }
    out
}

/// Status byte meanings (the second byte of the 6-byte status reply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// 206: the last packet was taken; the board accepts the next.
    Ok,
    /// 238: the board is busy; ask again.
    Busy,
    /// 207: the CRC did not match; send the packet again.
    Error,
    /// 236: the job has finished.
    Finish,
    /// 239: low power (the board's supply is too weak).
    Power,
    /// 204: serial number confirmed, or (M3) finished.
    SerialOkOrM3Finish,
    /// 0: nothing read from USB.
    NoReply,
    Unknown(u8),
}

impl Status {
    pub const OK: u8 = 206;
    pub const BUSY: u8 = 238;
    pub const ERROR: u8 = 207;
    pub const FINISH: u8 = 236;
    pub const POWER: u8 = 239;
    pub const SERIAL_OK_M3_FINISH: u8 = 204;

    pub fn from_byte(b: u8) -> Status {
        match b {
            Self::OK => Status::Ok,
            Self::BUSY => Status::Busy,
            Self::ERROR => Status::Error,
            Self::FINISH => Status::Finish,
            Self::POWER => Status::Power,
            Self::SERIAL_OK_M3_FINISH => Status::SerialOkOrM3Finish,
            0 => Status::NoReply,
            other => Status::Unknown(other),
        }
    }

    pub fn to_byte(self) -> u8 {
        match self {
            Status::Ok => Self::OK,
            Status::Busy => Self::BUSY,
            Status::Error => Self::ERROR,
            Status::Finish => Self::FINISH,
            Status::Power => Self::POWER,
            Status::SerialOkOrM3Finish => Self::SERIAL_OK_M3_FINISH,
            Status::NoReply => 0,
            Status::Unknown(b) => b,
        }
    }

    /// The board has nothing left to run (bit 0x02 clear). A sender waiting for the end of a
    /// job polls until this is true.
    pub fn is_idle_byte(b: u8) -> bool {
        b != 0 && b & 0x02 == 0
    }
}

/// Fixed command strings (EGV text, framed like any other).
pub mod commands {
    /// Move to the home corner.
    pub const HOME: &str = "IPP\n";
    /// Lock the rail (motors on).
    pub const LOCK_RAIL: &str = "IS1P\n";
    /// Unlock the rail (motors off; the head can be pushed by hand).
    pub const UNLOCK_RAIL: &str = "IS2P\n";
    /// Pause a running job.
    pub const PAUSE: &str = "PN!\n";
    /// Resume a paused job.
    pub const RESUME: &str = "PN&\n";
    /// Reset the board.
    pub const RESET: &str = "I*\n";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 32-entry nibble table the original sender uses for the same CRC.
    const NIBBLE_TABLE: [u8; 32] = [
        0x00, 0x5E, 0xBC, 0xE2, 0x61, 0x3F, 0xDD, 0x83, 0xC2, 0x9C, 0x7E, 0x20, 0xA3, 0xFD, 0x1F, 0x41, 0x00, 0x9D, 0x23, 0xBE, 0x46, 0xDB, 0x65,
        0xF8, 0x8C, 0x11, 0xAF, 0x32, 0xCA, 0x57, 0xE9, 0x74,
    ];

    fn crc_by_table(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in data {
            crc ^= b;
            crc = NIBBLE_TABLE[(crc & 0x0F) as usize] ^ NIBBLE_TABLE[16 + ((crc >> 4) & 0x0F) as usize];
        }
        crc
    }

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc8(b"123456789"), 0xA1);
        assert_eq!(crc8(b""), 0);
    }

    #[test]
    fn crc_matches_the_nibble_table_for_every_byte_pair() {
        for a in 0..=255u8 {
            for b in (0..=255u8).step_by(5) {
                assert_eq!(crc8(&[a, b]), crc_by_table(&[a, b]));
            }
        }
        let p = b"IBzzzvRzzzvS1PFFFFFFFFFFFFFFFF";
        assert_eq!(crc8(p), crc_by_table(p));
    }

    fn payload(f: &Frame) -> String {
        String::from_utf8(f.packet.unwrap().payload.to_vec()).unwrap()
    }

    #[test]
    fn line_ends_cut_and_pad_with_f() {
        let r = frames(b"IBzzzvRzzzvS1P\nICV2490731016000027CNLBS1EDz139Rz139Tz139Lz139FNSE-\n");
        assert_eq!(r.unsent, 0);
        assert_eq!(payload(&r.frames[0]), "IBzzzvRzzzvS1PFFFFFFFFFFFFFFFF");
        assert_eq!(r.frames[0].after, None);
        assert_eq!(payload(&r.frames[1]), "ICV2490731016000027CNLBS1EDz13");
        assert_eq!(payload(&r.frames[2]), "9Rz139Tz139Lz139FNSEFFFFFFFFFF");
        assert_eq!(r.frames[2].after, Some(After::WaitFinish));
        assert_eq!(r.frames.len(), 3);
    }

    #[test]
    fn a_packet_ending_in_p_gets_an_extra_f() {
        let r = frames(b"IPP\n");
        assert_eq!(payload(&r.frames[0]), format!("IPP{}", "F".repeat(27)));
    }

    #[test]
    fn a_lone_line_end_is_thirty_f() {
        let r = frames(b"\n");
        assert_eq!(payload(&r.frames[0]), "F".repeat(30));
    }

    #[test]
    fn meta_only_lines_send_nothing() {
        let r = frames(b"-\n");
        assert_eq!(r.frames, vec![Frame { packet: None, after: Some(After::WaitFinish) }]);
        let r = frames(b"*\n");
        assert_eq!(r.frames[0].after, Some(After::Abort));
        assert!(r.frames[0].packet.is_none());
    }

    #[test]
    fn meta_characters() {
        assert_eq!(frames(b"PN!\n").frames[0].after, Some(After::Pause));
        assert_eq!(frames(b"PN&\n").frames[0].after, Some(After::Resume));
        assert_eq!(frames(b"I\x18\n").frames[0].after, Some(After::Quit));
        let r = frames(b"IPP%\n");
        let p = r.frames[0].packet.unwrap();
        assert!(p.bad_crc);
        assert_eq!(p.crc(), crc8(&p.payload) ^ 0xFF);
        let r = frames(b"IX#\n");
        assert_eq!(payload(&r.frames[0]), format!("I{}", "X".repeat(29)));
        let r = frames(b"A1234\n");
        assert_eq!(r.frames[0].after, Some(After::ConfirmSerial));
    }

    #[test]
    fn at_packets_pad_with_zero() {
        let r = frames(b"AT1\n");
        let p = r.frames[0].packet.unwrap();
        assert_eq!(&p.payload[..3], b"AT1");
        assert!(p.payload[3..].iter().all(|&b| b == 0));
        assert_eq!(r.frames[0].after, None);
    }

    #[test]
    fn long_text_is_cut_every_thirty_bytes() {
        let text = "B".repeat(65) + "\n";
        let r = frames(text.as_bytes());
        assert_eq!(r.frames.len(), 3);
        assert_eq!(payload(&r.frames[0]), "B".repeat(30));
        assert_eq!(payload(&r.frames[2]), format!("BBBBB{}", "F".repeat(25)));
    }

    #[test]
    fn exactly_thirty_then_a_line_end_sends_an_extra_f_packet() {
        let text = "B".repeat(30) + "\n";
        let r = frames(text.as_bytes());
        assert_eq!(r.frames.len(), 2);
        assert_eq!(payload(&r.frames[1]), "F".repeat(30));
    }

    #[test]
    fn a_meta_character_at_the_thirtieth_byte_reaches_its_line_end() {
        let text = "B".repeat(29) + "-\n";
        let r = frames(text.as_bytes());
        assert_eq!(r.frames.len(), 1);
        assert_eq!(payload(&r.frames[0]), "B".repeat(29) + "F");
        assert_eq!(r.frames[0].after, Some(After::WaitFinish));
    }

    #[test]
    fn short_text_without_a_line_end_waits() {
        let r = frames(b"IBa");
        assert!(r.frames.is_empty());
        assert_eq!(r.unsent, 3);
        let text = "B".repeat(35);
        let r = frames(text.as_bytes());
        assert_eq!(r.frames.len(), 1);
        assert_eq!(r.unsent, 5);
    }

    #[test]
    fn wire_form_has_the_epp_byte_and_crc() {
        let r = frames(b"IPP\n");
        let p = r.frames[0].packet.unwrap();
        let w = p.to_wire();
        assert_eq!(w.len(), 33);
        assert_eq!(w[0], 0xA6);
        assert_eq!(w[1], 0x00);
        assert_eq!(&w[2..32], &p.payload);
        assert_eq!(w[32], crc8(&p.payload));
    }

    #[test]
    fn status_bytes() {
        for s in
            [Status::Ok, Status::Busy, Status::Error, Status::Finish, Status::Power, Status::SerialOkOrM3Finish, Status::NoReply, Status::Unknown(1)]
        {
            assert_eq!(Status::from_byte(s.to_byte()), s);
        }
        assert!(Status::is_idle_byte(Status::FINISH));
        assert!(Status::is_idle_byte(Status::SERIAL_OK_M3_FINISH));
        assert!(!Status::is_idle_byte(Status::BUSY));
        assert!(!Status::is_idle_byte(Status::OK));
        assert!(!Status::is_idle_byte(0));
    }
}
