//! MCC100 frame codec.
//!
//! Wire layout (owner's `04-controller-protocol.md` section 3.2, verified against logged frames):
//!
//! ```text
//! offset size field
//! 0      2    seq      u16 big-endian
//! 2      2    crc      CRC-16/MODBUS over bytes [4..]; the PC stores it high byte first, the card low byte first
//! 4      2    length   u16 big-endian = bytes after this field (unit + func + payload)
//! 6      1    unit     0x00
//! 7      1    func     0x20/0x21 stream, 0x26 byte block, 0x30 read, 0x40 write; func | 0x80 = exception
//! 8      ...  payload  all integers little-endian
//! ```
//!
//! There is no transport here: frames are plain byte vectors. [`Request`] can only hold a read
//! that passed [`check_read`] or a [`SafeWrite`]; the byte-block (0x26) and stream (0x20, 0x21)
//! functions are decoded for dissection ([`RawFrame`]) but have no request type, so a firmware
//! download or a stream cannot be produced by this crate's public API.

use crate::command::{SafeWrite, SafetyConfig};
use crate::crc::{CrcOrder, crc_bytes, crc_from_bytes, crc16_modbus};
use crate::registers::{Forbidden, check_read};
use std::fmt;

/// Byte stream (offline upload paths): `u16 count, count x u8`.
pub const FUNC_STREAM: u8 = 0x20;
/// Second byte-stream code; its reply falls into the vendor decoder's default case.
pub const FUNC_STREAM_ALT: u8 = 0x21;
/// Byte-block write: `u32 addr, u16 count, count x u8` (firmware / file download).
pub const FUNC_BYTE_BLOCK: u8 = 0x26;
/// Read registers: `u32 addr, u16 count`.
pub const FUNC_READ: u8 = 0x30;
/// Write registers: `u32 addr, u16 count, count x u32`.
pub const FUNC_WRITE: u8 = 0x40;
/// Reply function = request function | 0x80, followed by one exception byte.
pub const EXCEPTION_BIT: u8 = 0x80;
/// Bytes before the length-counted area.
pub const HEADER_LEN: usize = 6;
/// The vendor decoder rejects shorter frames.
pub const MIN_FRAME_LEN: usize = 8;
/// Receive buffer of the vendor driver (1448 bytes).
pub const MAX_FRAME_LEN: usize = 0x5A8;

/// Name of a function code.
pub fn func_name(func: u8) -> &'static str {
    match func & !EXCEPTION_BIT {
        FUNC_STREAM => "STREAM20",
        FUNC_STREAM_ALT => "STREAM21",
        FUNC_BYTE_BLOCK => "BYTEBLOCK",
        FUNC_READ => "READ",
        FUNC_WRITE => "WRITE",
        _ => "UNKNOWN",
    }
}

/// Driver ErrCode of an exception reply: `500 + code`.
pub fn exception_errcode(code: u8) -> u32 {
    500 + code as u32
}

/// A decode failure with the vendor DLL's return code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    /// Return code: 2 short, 4 sequence mismatch, 5 / 6 length inconsistent.
    pub rc: u8,
    /// What went wrong.
    pub message: String,
}

impl DecodeError {
    fn new(rc: u8, message: impl Into<String>) -> DecodeError {
        DecodeError { rc, message: message.into() }
    }

    /// The ErrCode the vendor driver logs: `2000 + rc`.
    pub fn errcode(&self) -> u32 {
        2000 + self.rc as u32
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode error rc={}: {}", self.rc, self.message)
    }
}

impl std::error::Error for DecodeError {}

/// Why a request could not be decoded or built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// Framing problem.
    Frame(DecodeError),
    /// CRC does not match in the PC (high byte first) order.
    BadCrc,
    /// Length field disagrees with the datagram.
    BadLength,
    /// The request is a forbidden register or vector, or an unsupported function.
    Forbidden(Forbidden),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestError::Frame(e) => write!(f, "{e}"),
            RequestError::BadCrc => write!(f, "request CRC mismatch"),
            RequestError::BadLength => write!(f, "request length field mismatch"),
            RequestError::Forbidden(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RequestError {}

impl From<Forbidden> for RequestError {
    fn from(f: Forbidden) -> RequestError {
        RequestError::Forbidden(f)
    }
}

/// Build `[seq][crc][len][unit][func][payload]`. Crate-private: only typed requests and replies
/// reach the wire format.
pub(crate) fn build_frame(seq: u16, func: u8, payload: &[u8], order: CrcOrder) -> Result<Vec<u8>, DecodeError> {
    let body_len = 2 + payload.len();
    if body_len > 0xFFFF || HEADER_LEN + body_len > MAX_FRAME_LEN {
        return Err(DecodeError::new(5, format!("frame of {} bytes exceeds {MAX_FRAME_LEN}", HEADER_LEN + body_len)));
    }
    let mut tail = Vec::with_capacity(2 + body_len);
    tail.extend_from_slice(&(body_len as u16).to_be_bytes());
    tail.push(0); // unit
    tail.push(func);
    tail.extend_from_slice(payload);
    let crc = crc16_modbus(&tail);
    let mut out = Vec::with_capacity(4 + tail.len());
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&crc_bytes(crc, order));
    out.extend_from_slice(&tail);
    Ok(out)
}

/// Header-level view of one datagram. Never fails on malformed input: missing fields are `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    /// The datagram.
    pub raw: Vec<u8>,
    /// Sequence number.
    pub seq: Option<u16>,
    /// The two stored CRC bytes.
    pub crc_field: Option<[u8; 2]>,
    /// Length field.
    pub length: Option<u16>,
    /// Unit byte.
    pub unit: Option<u8>,
    /// Function byte.
    pub func: Option<u8>,
    /// Bytes after the function byte.
    pub payload: Vec<u8>,
    /// CRC computed over bytes `[4..]`.
    pub crc_value: Option<u16>,
    /// Order in which the stored CRC matches, if any.
    pub crc_order: Option<CrcOrder>,
}

impl RawFrame {
    /// Split a datagram into header fields and check the CRC in both byte orders. If both match
    /// (the two bytes are equal) the PC order is reported.
    pub fn parse(raw: &[u8]) -> RawFrame {
        let seq = (raw.len() >= 2).then(|| u16::from_be_bytes([raw[0], raw[1]]));
        let crc_field = (raw.len() >= 4).then(|| [raw[2], raw[3]]);
        let length = (raw.len() >= 6).then(|| u16::from_be_bytes([raw[4], raw[5]]));
        let unit = raw.get(6).copied();
        let func = raw.get(7).copied();
        let (crc_value, crc_order) = match crc_field {
            Some(f) => {
                let v = crc16_modbus(&raw[4..]);
                let order = [CrcOrder::HiFirst, CrcOrder::LoFirst].into_iter().find(|&o| crc_from_bytes(f, o) == v);
                (Some(v), order)
            }
            None => (None, None),
        };
        RawFrame { raw: raw.to_vec(), seq, crc_field, length, unit, func, payload: raw.get(8..).unwrap_or(&[]).to_vec(), crc_value, crc_order }
    }

    /// True if the stored CRC matches in either order.
    pub fn crc_ok(&self) -> bool {
        self.crc_order.is_some()
    }

    /// True if the length field equals the bytes after it.
    pub fn length_ok(&self) -> bool {
        self.length.is_some_and(|l| l as usize == self.raw.len().saturating_sub(HEADER_LEN))
    }

    /// True if the function byte has the exception bit.
    pub fn is_exception(&self) -> bool {
        self.func.is_some_and(|f| f & EXCEPTION_BIT != 0)
    }

    /// Exception byte of an exception reply.
    pub fn exception_code(&self) -> Option<u8> {
        if self.is_exception() { self.payload.first().copied() } else { None }
    }
}

/// A validated read request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRequest {
    addr: u32,
    count: u16,
}

impl ReadRequest {
    /// Check the range against the read allow-list.
    pub fn new(addr: u32, count: u16) -> Result<ReadRequest, Forbidden> {
        check_read(addr, count as u32)?;
        Ok(ReadRequest { addr, count })
    }

    /// First register.
    pub fn addr(&self) -> u32 {
        self.addr
    }

    /// Number of registers.
    pub fn count(&self) -> u16 {
        self.count
    }
}

/// A request that may go to the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read registers.
    Read(ReadRequest),
    /// Write through a validated [`SafeWrite`].
    Write(SafeWrite),
}

impl Request {
    /// Read request for an allowed range.
    pub fn read(addr: u32, count: u16) -> Result<Request, Forbidden> {
        Ok(Request::Read(ReadRequest::new(addr, count)?))
    }

    /// Wrap a validated write.
    pub fn write(w: SafeWrite) -> Request {
        Request::Write(w)
    }

    /// Function code.
    pub fn func(&self) -> u8 {
        match self {
            Request::Read(_) => FUNC_READ,
            Request::Write(_) => FUNC_WRITE,
        }
    }

    /// Vendor log vector `[func, addr, count, data...]`.
    pub fn to_vector(&self) -> Vec<u32> {
        match self {
            Request::Read(r) => vec![FUNC_READ as u32, r.addr, r.count as u32],
            Request::Write(w) => w.request_vector(),
        }
    }

    /// Encode as a datagram with the PC's CRC order.
    pub fn encode(&self, seq: u16) -> Result<Vec<u8>, DecodeError> {
        let mut payload = Vec::new();
        match self {
            Request::Read(r) => {
                payload.extend_from_slice(&r.addr.to_le_bytes());
                payload.extend_from_slice(&r.count.to_le_bytes());
                build_frame(seq, FUNC_READ, &payload, CrcOrder::HiFirst)
            }
            Request::Write(w) => {
                payload.extend_from_slice(&w.register().to_le_bytes());
                payload.extend_from_slice(&(w.words().len() as u16).to_le_bytes());
                for word in w.words() {
                    payload.extend_from_slice(&word.to_le_bytes());
                }
                build_frame(seq, FUNC_WRITE, &payload, CrcOrder::HiFirst)
            }
        }
    }

    /// Decode a PC datagram. Fails for bad CRC or length, for functions other than read and write,
    /// and for any forbidden register, vector or laser data.
    pub fn decode(frame: &[u8]) -> Result<(u16, Request), RequestError> {
        let (seq, req) = decode_request(frame, &SafetyConfig::default(), None, false)?;
        Ok((seq, req))
    }
}

fn le_words(bytes: &[u8]) -> Vec<u32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

/// Shared decoder. With `allow_laser` the simulator can see laser-class writes (so it can refuse
/// them through its gate model); the public decoder never allows them.
pub(crate) fn decode_request(frame: &[u8], cfg: &SafetyConfig, do_word: Option<u32>, allow_laser: bool) -> Result<(u16, Request), RequestError> {
    if frame.len() < MIN_FRAME_LEN {
        return Err(RequestError::Frame(DecodeError::new(2, format!("request frame of {} bytes is shorter than 8", frame.len()))));
    }
    let raw = RawFrame::parse(frame);
    if raw.crc_order != Some(CrcOrder::HiFirst) {
        return Err(RequestError::BadCrc);
    }
    if !raw.length_ok() {
        return Err(RequestError::BadLength);
    }
    let seq = raw.seq.unwrap_or(0);
    let func = raw.func.unwrap_or(0);
    let p = &raw.payload;
    match func {
        FUNC_READ => {
            if p.len() != 6 {
                return Err(RequestError::Frame(DecodeError::new(5, "READ request payload must be 6 bytes")));
            }
            let addr = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            let count = u16::from_le_bytes([p[4], p[5]]);
            Ok((seq, Request::read(addr, count)?))
        }
        FUNC_WRITE => {
            if p.len() < 6 {
                return Err(RequestError::Frame(DecodeError::new(5, "payload shorter than addr+count")));
            }
            let addr = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            let count = u16::from_le_bytes([p[4], p[5]]) as usize;
            let rest = &p[6..];
            if rest.len() != 4 * count {
                return Err(RequestError::Frame(DecodeError::new(5, format!("WRITE count {count} but {} data bytes", rest.len()))));
            }
            let words = le_words(rest);
            let w = if allow_laser { SafeWrite::from_raw_for_simulator(addr, &words, cfg, do_word)? } else { SafeWrite::try_from_raw(addr, &words)? };
            Ok((seq, Request::Write(w)))
        }
        other => Err(RequestError::Forbidden(Forbidden::Function(other))),
    }
}

/// A card reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Read reply: address and words.
    Read {
        /// First register.
        addr: u32,
        /// Values.
        words: Vec<u32>,
    },
    /// Echo of a write (0x40) or byte block (0x26).
    Write {
        /// Function that is echoed.
        func: u8,
        /// Address.
        addr: u32,
        /// Count.
        count: u16,
    },
    /// Exception reply.
    Exception {
        /// Request function (without the exception bit).
        func: u8,
        /// Exception byte; the driver logs `ErrCode = 500 + code`.
        code: u8,
    },
    /// A reply the vendor decoder treats as default: `[func, first payload byte]`.
    Other {
        /// Function byte.
        func: u8,
        /// First payload byte.
        byte: u8,
    },
}

impl Reply {
    /// Encode as the card does (CRC low byte first). The success layouts are derived from the PC
    /// decoder, not seen on the wire; only exception replies were observed.
    pub fn encode(&self, seq: u16) -> Result<Vec<u8>, DecodeError> {
        match self {
            Reply::Read { addr, words } => {
                let mut p = Vec::with_capacity(6 + 4 * words.len());
                p.extend_from_slice(&addr.to_le_bytes());
                p.extend_from_slice(&(words.len() as u16).to_le_bytes());
                for w in words {
                    p.extend_from_slice(&w.to_le_bytes());
                }
                build_frame(seq, FUNC_READ, &p, CrcOrder::LoFirst)
            }
            Reply::Write { func, addr, count } => {
                let mut p = Vec::with_capacity(6);
                p.extend_from_slice(&addr.to_le_bytes());
                p.extend_from_slice(&count.to_le_bytes());
                build_frame(seq, *func, &p, CrcOrder::LoFirst)
            }
            Reply::Exception { func, code } => build_frame(seq, func | EXCEPTION_BIT, &[*code], CrcOrder::LoFirst),
            Reply::Other { func, byte } => build_frame(seq, *func, &[*byte], CrcOrder::LoFirst),
        }
    }

    /// Decode like the vendor `decodeWithSeq`: the CRC, length field and unit byte are ignored; a
    /// sequence mismatch is rc 4. `seq = None` skips the sequence check.
    pub fn decode(frame: &[u8], seq: Option<u16>) -> Result<Reply, DecodeError> {
        if frame.len() < MIN_FRAME_LEN {
            return Err(DecodeError::new(2, format!("reply frame of {} bytes is shorter than 8", frame.len())));
        }
        let got = u16::from_be_bytes([frame[0], frame[1]]);
        if let Some(want) = seq
            && got != want
        {
            return Err(DecodeError::new(4, format!("seq mismatch: got {got:#06x}, want {want:#06x}")));
        }
        let func = frame[7];
        let size = frame.len();
        let need = |n: usize| -> Result<(), DecodeError> {
            if size < n { Err(DecodeError::new(2, format!("func {func:#04x} reply needs {n} bytes, got {size}"))) } else { Ok(()) }
        };
        if func & EXCEPTION_BIT != 0 {
            need(9)?;
            return Ok(Reply::Exception { func: func & !EXCEPTION_BIT, code: frame[8] });
        }
        match func {
            FUNC_READ => {
                need(14)?;
                let addr = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
                let count = u16::from_le_bytes([frame[12], frame[13]]) as usize;
                if 4 * count != size - 14 {
                    return Err(DecodeError::new(5, format!("READ reply count {count} but {} data bytes", size - 14)));
                }
                let words = le_words(&frame[14..]);
                Ok(Reply::Read { addr, words })
            }
            FUNC_WRITE | FUNC_BYTE_BLOCK => {
                need(14)?;
                let addr = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]);
                let count = u16::from_le_bytes([frame[12], frame[13]]);
                Ok(Reply::Write { func, addr, count })
            }
            FUNC_STREAM => {
                need(10)?;
                let count = u16::from_le_bytes([frame[8], frame[9]]) as usize;
                if count != size - 10 {
                    return Err(DecodeError::new(6, format!("STREAM reply count {count} but {} data bytes", size - 10)));
                }
                Ok(Reply::Other { func, byte: frame.get(10).copied().unwrap_or(0) })
            }
            _ => {
                need(9)?;
                Ok(Reply::Other { func, byte: frame[8] })
            }
        }
    }

    /// Vendor `Data:` log vector: `[func, addr, count]` for a write echo, `[func, addr, count,
    /// words...]` for a read, `[func | 0x80, code]` for an exception.
    pub fn to_vector(&self) -> Vec<u32> {
        match self {
            Reply::Read { addr, words } => {
                let mut v = vec![FUNC_READ as u32, *addr, words.len() as u32];
                v.extend_from_slice(words);
                v
            }
            Reply::Write { func, addr, count } => vec![*func as u32, *addr, *count as u32],
            Reply::Exception { func, code } => vec![(func | EXCEPTION_BIT) as u32, *code as u32],
            Reply::Other { func, byte } => vec![*func as u32, *byte as u32],
        }
    }
}

/// Encode a byte-block request. Crate-private on purpose (firmware / file download is never sent).
#[cfg(test)]
pub(crate) fn encode_byte_block(seq: u16, addr: u32, data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut p = Vec::new();
    p.extend_from_slice(&addr.to_le_bytes());
    p.extend_from_slice(&(data.len() as u16).to_le_bytes());
    p.extend_from_slice(data);
    build_frame(seq, FUNC_BYTE_BLOCK, &p, CrcOrder::HiFirst)
}

/// Encode a stream request (0x20 / 0x21). Crate-private on purpose.
#[cfg(test)]
pub(crate) fn encode_stream(seq: u16, func: u8, data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut p = Vec::new();
    p.extend_from_slice(&(data.len() as u16).to_le_bytes());
    p.extend_from_slice(data);
    build_frame(seq, func, &p, CrcOrder::HiFirst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;
    use crate::fifo::FifoItem;

    /// Logged `sendto`: READ 1000 x 2 (owner's `08 section 2.2`).
    const SENDTO: [u8; 14] = [0x00, 0x00, 0x8c, 0x75, 0x00, 0x08, 0x00, 0x30, 0xe8, 0x03, 0x00, 0x00, 0x02, 0x00];
    const EXC_503: [u8; 9] = [0x1d, 0x3f, 0x34, 0x45, 0x00, 0x03, 0x00, 0xc0, 0x03];
    const EXC_502_READ: [u8; 9] = [0x06, 0xc5, 0xd0, 0x45, 0x00, 0x03, 0x00, 0xb0, 0x02];
    const EXC_502_WRITE: [u8; 9] = [0x61, 0x09, 0xf5, 0x85, 0x00, 0x03, 0x00, 0xc0, 0x02];

    #[test]
    fn read_request_matches_the_logged_frame() {
        let r = Request::read(1000, 2).unwrap();
        assert_eq!(r.encode(0).unwrap(), SENDTO);
        assert_eq!(r.to_vector(), vec![0x30, 1000, 2]);
        let (seq, back) = Request::decode(&SENDTO).unwrap();
        assert_eq!(seq, 0);
        assert_eq!(back, r);
    }

    #[test]
    fn logged_exception_replies() {
        for (frame, seq, func, code, errcode) in
            [(EXC_503, 0x1d3fu16, 0x40u8, 3u8, 503u32), (EXC_502_READ, 0x06c5, 0x30, 2, 502), (EXC_502_WRITE, 0x6109, 0x40, 2, 502)]
        {
            let rep = Reply::Exception { func, code };
            assert_eq!(rep.encode(seq).unwrap(), frame, "encode");
            assert_eq!(Reply::decode(&frame, Some(seq)).unwrap(), rep);
            assert_eq!(exception_errcode(code), errcode);
            assert_eq!(rep.to_vector(), vec![(func | 0x80) as u32, code as u32]);
            let raw = RawFrame::parse(&frame);
            assert!(raw.crc_ok() && raw.length_ok());
            assert_eq!(raw.crc_order, Some(CrcOrder::LoFirst));
            assert_eq!(raw.exception_code(), Some(code));
        }
    }

    #[test]
    fn decode_reply_errors_carry_the_dll_codes() {
        let e = Reply::decode(&EXC_503, Some(0x1d40)).unwrap_err();
        assert_eq!((e.rc, e.errcode()), (4, 2004));
        let e = Reply::decode(&EXC_503[..7], None).unwrap_err();
        assert_eq!(e.rc, 2);
        // a READ reply with a wrong count
        let mut bad = Reply::Read { addr: 1000, words: vec![1, 2] }.encode(1).unwrap();
        bad.pop();
        assert_eq!(Reply::decode(&bad, Some(1)).unwrap_err().rc, 5);
        // a READ reply shorter than its fixed header
        let short = Reply::Read { addr: 1000, words: vec![] }.encode(1).unwrap();
        assert!(Reply::decode(&short[..12], Some(1)).is_err());
    }

    #[test]
    fn command_write_logged_vector() {
        // DataEx:40 65 06 03 01 c350 176f ea56 3d0900 answered by EXC_503 (seq 0x1d3f)
        let w = SafeWrite::try_from_raw(0x65, &[3, 1, 0xC350, 0x176F, 0xEA56, 0x3D0900]).unwrap();
        let frame = Request::write(w.clone()).encode(0x1D3F).unwrap();
        assert_eq!(&frame[..2], &[0x1d, 0x3f]);
        assert_eq!(u16::from_be_bytes([frame[4], frame[5]]) as usize, 8 + 4 * 6);
        assert_eq!(frame.len(), 6 + 8 + 24);
        assert_eq!(&frame[6..8], &[0x00, 0x40]);
        assert_eq!(&frame[8..14], &[0x65, 0, 0, 0, 6, 0]);
        let crc = crc16_modbus(&frame[4..]);
        assert_eq!(&frame[2..4], &[(crc >> 8) as u8, crc as u8], "PC order: high byte first");
        assert_eq!(Request::decode(&frame).unwrap(), (0x1D3F, Request::Write(w.clone())));
        assert_eq!(w.request_vector(), vec![0x40, 0x65, 6, 3, 1, 0xC350, 0x176F, 0xEA56, 0x3D0900]);
    }

    #[test]
    fn negative_words_are_two_complement() {
        let w = command::jog_step(0, -4000.0, &command::MachineParams::default()).unwrap();
        let (_, Request::Write(back)) = Request::decode(&Request::write(w.clone()).encode(1).unwrap()).unwrap() else { panic!() };
        assert_eq!(back.words()[5], (-4_000_000i32) as u32);
    }

    #[test]
    fn fifo_frame_size_is_1206_bytes() {
        // 298 words = 1192 bytes; + u32 addr + u16 count + 8-byte header = 1206 bytes
        let items: Vec<FifoItem> = (0..99).map(|_| FifoItem::tick(0, 0)).collect();
        let w = command::fifo_frame(9, &items).unwrap();
        assert_eq!(w.words().len(), 298);
        let frame = Request::write(w).encode(9).unwrap();
        assert_eq!(frame.len(), 1206);
        assert!(frame.len() <= MAX_FRAME_LEN);
        assert_eq!(u16::from_be_bytes([frame[4], frame[5]]), 1200);
    }

    #[test]
    fn oversized_frames_are_refused() {
        let items: Vec<FifoItem> = (0..130).map(|_| FifoItem::tick(0, 0)).collect();
        let w = command::fifo_frame(9, &items).unwrap();
        assert!(Request::write(w).encode(1).is_err());
    }

    #[test]
    fn read_replies_round_trip() {
        let rep = Reply::Read { addr: 10000, words: (0..18).collect() };
        let f = rep.encode(0x0102).unwrap();
        assert_eq!(f.len(), 14 + 72);
        assert_eq!(Reply::decode(&f, Some(0x0102)).unwrap(), rep);
        assert_eq!(rep.to_vector()[..3], [0x30, 10000, 18]);
        let w = Reply::Write { func: FUNC_WRITE, addr: 0x65, count: 6 };
        assert_eq!(Reply::decode(&w.encode(5).unwrap(), Some(5)).unwrap(), w);
        let b = Reply::Write { func: FUNC_BYTE_BLOCK, addr: 0x1234, count: 3 };
        assert_eq!(Reply::decode(&b.encode(5).unwrap(), None).unwrap(), b);
    }

    #[test]
    fn byte_block_and_streams_decode_for_dissection_only() {
        let f = encode_byte_block(3, 0x1234, &[1, 2, 0xff]).unwrap();
        assert_eq!(u16::from_be_bytes([f[4], f[5]]), 3 + 3 + 5);
        let raw = RawFrame::parse(&f);
        assert_eq!(raw.func, Some(FUNC_BYTE_BLOCK));
        assert!(raw.crc_ok() && raw.length_ok());
        assert_eq!(func_name(raw.func.unwrap()), "BYTEBLOCK");
        for func in [FUNC_STREAM, FUNC_STREAM_ALT] {
            let f = encode_stream(4, func, b"abc").unwrap();
            assert_eq!(u16::from_be_bytes([f[4], f[5]]), 2 + 2 + 3);
            assert_eq!(&f[8..], &[3, 0, b'a', b'b', b'c']);
            // and the public decoder refuses to turn any of them into a request
            assert_eq!(Request::decode(&f), Err(RequestError::Forbidden(Forbidden::Function(func))));
        }
        let f = encode_byte_block(3, 0x1234, &[1]).unwrap();
        assert_eq!(Request::decode(&f), Err(RequestError::Forbidden(Forbidden::Function(FUNC_BYTE_BLOCK))));
    }

    #[test]
    fn request_decoder_rejects_damage() {
        // short
        assert!(matches!(Request::decode(&SENDTO[..7]), Err(RequestError::Frame(_))));
        // bad CRC
        let mut f = SENDTO;
        f[10] ^= 1;
        assert_eq!(Request::decode(&f), Err(RequestError::BadCrc));
        // card order is not accepted as a request
        let lo = build_frame(0, FUNC_READ, &[0xe8, 3, 0, 0, 2, 0], CrcOrder::LoFirst).unwrap();
        assert_eq!(Request::decode(&lo), Err(RequestError::BadCrc));
        // length lies (re-CRC'd so only the length is wrong)
        let mut f = SENDTO;
        f[5] = 9;
        let crc = crc16_modbus(&f[4..]);
        f[2] = (crc >> 8) as u8;
        f[3] = crc as u8;
        assert_eq!(Request::decode(&f), Err(RequestError::BadLength));
        // read of a licence register
        let lic = build_frame(0, FUNC_READ, &[0x6c, 0xe8, 0, 0, 1, 0], CrcOrder::HiFirst).unwrap();
        assert!(matches!(Request::decode(&lic), Err(RequestError::Forbidden(Forbidden::LicenceArea(_)))));
        // a hand-built write to register 150 never decodes
        let mut p = Vec::new();
        p.extend_from_slice(&150u32.to_le_bytes());
        p.extend_from_slice(&1u16.to_le_bytes());
        p.extend_from_slice(&5555u32.to_le_bytes());
        let f = build_frame(0, FUNC_WRITE, &p, CrcOrder::HiFirst).unwrap();
        assert!(matches!(Request::decode(&f), Err(RequestError::Forbidden(Forbidden::Register150Or151(150)))));
        // ... nor 0x65 <- [0x65]
        let mut p = Vec::new();
        p.extend_from_slice(&0x65u32.to_le_bytes());
        p.extend_from_slice(&1u16.to_le_bytes());
        p.extend_from_slice(&0x65u32.to_le_bytes());
        let f = build_frame(0, FUNC_WRITE, &p, CrcOrder::HiFirst).unwrap();
        assert!(matches!(Request::decode(&f), Err(RequestError::Forbidden(Forbidden::Command(_)))));
        // count lies
        let mut p = Vec::new();
        p.extend_from_slice(&0x67u32.to_le_bytes());
        p.extend_from_slice(&2u16.to_le_bytes());
        p.extend_from_slice(&3u32.to_le_bytes());
        let f = build_frame(0, FUNC_WRITE, &p, CrcOrder::HiFirst).unwrap();
        assert!(matches!(Request::decode(&f), Err(RequestError::Frame(_))));
    }

    #[test]
    fn unknown_functions_build_nothing_decodable() {
        let f = build_frame(1, 0x55, &[1, 2], CrcOrder::HiFirst).unwrap();
        assert_eq!(Request::decode(&f), Err(RequestError::Forbidden(Forbidden::Function(0x55))));
        assert_eq!(func_name(0x55), "UNKNOWN");
        assert_eq!(func_name(0xb0), "READ");
    }

    #[test]
    fn tolerant_parse_never_panics() {
        for n in 0..16 {
            let raw: Vec<u8> = (0..n).map(|i| (i * 37 + 5) as u8).collect();
            let f = RawFrame::parse(&raw);
            assert_eq!(f.seq.is_some(), n >= 2);
            let _ = f.crc_ok();
            let _ = f.length_ok();
            let _ = Request::decode(&raw);
            let _ = Reply::decode(&raw, None);
        }
    }
}
