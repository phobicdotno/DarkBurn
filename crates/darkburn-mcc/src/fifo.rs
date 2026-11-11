//! FIFO items, frame packing and flow control for register 0x66.
//!
//! Item grammar (owner's `11-static-findings.md` section 5.2): `header = (payload_bytes << 16) | opcode`
//! followed by `payload_bytes / 4` words. The motion item is opcode 3000, a 12-byte tick:
//! `[0x00080BB8, (dY << 16) | dX, (freq << 16) | duty]` with `dX`, `dY` signed 16-bit motor pulses.
//!
//! A frame is `WRITE 0x66` with words `[frame_id, items...]`; the packer closes a frame when the
//! vector `[0x40, 0x66, count, frame_id, items...]` reaches 300 words (99 ticks) or at the end of a
//! batch. [`FifoFeeder`] reproduces the vendor flow control: frame id is register 1015 plus one, a
//! frame is sent only if `frame_bytes + 2000 <= register 1016`, at most 50 frames per pass.
//!
//! Public items can only be built as *dry* items (zero duty ticks, boundary and mode records). The
//! full grammar can be decoded for the simulator, but a frame carrying laser data or any
//! out-of-grammar item is refused by [`crate::command::SafeWrite`].

use crate::command::Policy;
use crate::registers::{FIFO_MARGIN_EMPTY, FIFO_SEND_HEADROOM_BYTES, Forbidden};
use std::collections::VecDeque;

/// Tick opcode.
pub const OP_TICK: u16 = 3000;
/// Boundary record, inserted when a control record follows a tick.
pub const OP_BOUNDARY: u16 = 3001;
/// Mode record (5 before laser-on, 4 after; meaning low confidence).
pub const OP_MODE: u16 = 3002;
/// In-stream 0x65-style record (DO, DA, PWM).
pub const OP_MISC: u16 = 9999;
/// Default pulse frequency word of a dry tick (the vendor treats 0 as 5000).
pub const DRY_TICK_FREQ: u16 = 5000;
/// Words per 3000 tick including its header (12 bytes).
pub const TICK_WORDS: usize = 3;
/// Words of the vendor vector prefix `[0x40, 0x66, count, frame_id]`.
pub const PREFIX_WORDS: usize = 4;
/// A frame closes at this many words including the prefix.
pub const FLUSH_WORDS: usize = 300;
/// At most this many frames per fill pass.
pub const MAX_FRAMES_PER_FILL: usize = 50;

/// Payload words each known opcode may carry.
fn arg_words(op: u16) -> Option<&'static [usize]> {
    Some(match op {
        3000 => &[2],
        3001 => &[0],
        3002 => &[1],
        9999 => &[3],
        103 | 109 => &[2],
        104 => &[2, 8],
        105 | 108 => &[4],
        106 => &[6],
        118 => &[3],
        2001 => &[2],
        _ => return None,
    })
}

/// One decoded 3000 tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// X pulses (signed 16 bit).
    pub dx: i16,
    /// Y pulses (signed 16 bit).
    pub dy: i16,
    /// Pulse frequency (0 is treated as 5000 by the vendor).
    pub freq: u16,
    /// Duty. Always 0 for items this crate builds.
    pub duty: u16,
}

/// One FIFO item. The fields are private: the only public constructors build dry items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoItem {
    opcode: u16,
    payload: Vec<u32>,
}

impl FifoItem {
    /// A dry motion tick: moves `dx`, `dy` pulses with duty 0.
    pub fn tick(dx: i16, dy: i16) -> FifoItem {
        FifoItem { opcode: OP_TICK, payload: vec![((dy as u16 as u32) << 16) | dx as u16 as u32, ((DRY_TICK_FREQ as u32) << 16)] }
    }

    /// A boundary record.
    pub fn boundary() -> FifoItem {
        FifoItem { opcode: OP_BOUNDARY, payload: vec![] }
    }

    /// A mode record. Only 4 and 5 pass the frame check; any other value is refused there.
    pub fn mode(mode: u32) -> FifoItem {
        FifoItem { opcode: OP_MODE, payload: vec![mode] }
    }

    /// Opcode.
    pub fn opcode(&self) -> u16 {
        self.opcode
    }

    /// Payload words.
    pub fn payload(&self) -> &[u32] {
        &self.payload
    }

    /// Header word `(payload_bytes << 16) | opcode`.
    pub fn header(&self) -> u32 {
        (((self.payload.len() * 4) as u32) << 16) | self.opcode as u32
    }

    /// Header plus payload.
    pub fn words(&self) -> Vec<u32> {
        let mut w = Vec::with_capacity(1 + self.payload.len());
        w.push(self.header());
        w.extend_from_slice(&self.payload);
        w
    }

    /// Number of words including the header.
    pub fn len_words(&self) -> usize {
        1 + self.payload.len()
    }

    /// The tick view of a 3000 item.
    pub fn as_tick(&self) -> Option<Tick> {
        if self.opcode != OP_TICK || self.payload.len() != 2 {
            return None;
        }
        let p0 = self.payload[0];
        let p1 = self.payload[1];
        Some(Tick { dx: p0 as u16 as i16, dy: (p0 >> 16) as u16 as i16, freq: (p1 >> 16) as u16, duty: p1 as u16 })
    }

    /// True if the item carries laser data. Such an item is decodable (so the simulator can model
    /// the vendor gate) but no public constructor creates one and [`crate::command::SafeWrite`]
    /// refuses frames containing one.
    pub fn is_laser(&self) -> bool {
        match self.opcode {
            OP_TICK => self.as_tick().is_some_and(|t| t.duty != 0),
            OP_MISC => match self.payload.first() {
                // DO: any laser port bit switched on (5 fibre gate, 6 pointer, 9 CO2).
                Some(2) => self.payload.get(2).is_some_and(|v| v & LASER_DO_MASK != 0),
                // PWM: always laser data in a stream.
                Some(3) | Some(0x11) => true,
                // DA: non-zero value.
                Some(4) => self.payload.get(2).is_some_and(|v| *v != 0),
                _ => false,
            },
            2002 | 2004 | 3 => true,
            _ => false,
        }
    }
}

/// DO bits for laser ports 5, 6 and 9.
const LASER_DO_MASK: u32 = (1 << 4) | (1 << 5) | (1 << 8);
/// DO bits the stream may address at all (laser ports plus gas and lamp ports 1, 2, 3, 7).
const ALLOWED_DO_MASK: u32 = LASER_DO_MASK | 0b100_0111;

/// Why FIFO words could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoParseError(pub String);

impl std::fmt::Display for FifoParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FIFO parse error: {}", self.0)
    }
}

impl std::error::Error for FifoParseError {}

/// A parsed frame: id and items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoFrame {
    /// Frame id (register 1015 plus one at send time).
    pub frame_id: u32,
    /// Items in order.
    pub items: Vec<FifoItem>,
}

/// Parse `[frame_id, items...]` strictly: known opcodes, exact payload lengths, no remainder.
pub fn parse_fifo_words(words: &[u32]) -> Result<FifoFrame, FifoParseError> {
    let (&frame_id, rest) = words.split_first().ok_or_else(|| FifoParseError("empty frame".into()))?;
    let mut items = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let header = rest[i];
        let op = (header & 0xFFFF) as u16;
        let bytes = (header >> 16) as usize;
        if !bytes.is_multiple_of(4) {
            return Err(FifoParseError(format!("item header {header:#x}: payload bytes not a multiple of 4")));
        }
        let n = bytes / 4;
        let allowed = match arg_words(op) {
            Some(a) => a,
            None if matches!(op, 2002 | 2004 | 3) => &[0, 1, 2, 3, 4][..],
            None => return Err(FifoParseError(format!("opcode {op} not in the grammar"))),
        };
        if !allowed.contains(&n) {
            return Err(FifoParseError(format!("opcode {op} with {n} payload words")));
        }
        if i + 1 + n > rest.len() {
            return Err(FifoParseError("item runs past the end of the frame".into()));
        }
        items.push(FifoItem { opcode: op, payload: rest[i + 1..i + 1 + n].to_vec() });
        i += 1 + n;
    }
    Ok(FifoFrame { frame_id, items })
}

/// Classify a parsed frame for the gate: `Motion` for dry content, `Laser` if any item carries
/// laser data or is a follower or pierce record, or an error for items outside the allow-list.
pub fn classify_frame(frame: &FifoFrame) -> Result<Policy, Forbidden> {
    let mut laser = false;
    for item in &frame.items {
        if item.opcode == OP_MISC {
            let sub = item.payload[0];
            match sub {
                2 => {
                    if (item.payload[1] | item.payload[2]) & !ALLOWED_DO_MASK != 0 {
                        return Err(Forbidden::FifoFrame(format!("DO record {:?} addresses a port outside the allow-list", item.payload)));
                    }
                }
                3 | 4 | 0x11 => {}
                13 => {
                    if item.payload[2] != 0 {
                        return Err(Forbidden::FifoFrame("extended DO on".into()));
                    }
                }
                _ => return Err(Forbidden::FifoFrame(format!("9999 sub-record {sub} not allowed"))),
            }
        }
        // A mode record carries one word, and only the two values seen in a stream.
        if item.opcode == OP_MODE && !matches!(item.payload[..], [4] | [5]) {
            return Err(Forbidden::FifoFrame(format!("mode record {:?} not allowed (only 4 and 5)", item.payload)));
        }
        // Follower moves, follow and pierce records (103..=109, 118) and 2001 belong to the
        // vendor's height-follower and pierce logic. Like laser data they decode, so a simulator
        // can model them, but they take the level no public constructor reaches.
        let vendor_only = !matches!(item.opcode, OP_TICK | OP_BOUNDARY | OP_MODE | OP_MISC);
        if item.is_laser() || vendor_only {
            laser = true;
        }
    }
    Ok(if laser { Policy::Laser } else { Policy::Motion })
}

/// One closed frame: the item words after the frame id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedFrame {
    /// Item words (headers and payloads), without the frame id.
    pub data: Vec<u32>,
    /// Items in the frame.
    pub items: usize,
}

impl PackedFrame {
    /// Words of the vendor vector `[0x40, 0x66, count, id, data...]`.
    pub fn vector_words(&self) -> usize {
        PREFIX_WORDS + self.data.len()
    }

    /// PC-side size used for the register 1016 test (1204 for 99 ticks).
    pub fn byte_size(&self) -> u32 {
        (self.vector_words() * 4) as u32
    }

    /// Count word: frame id plus data words.
    pub fn count(&self) -> usize {
        1 + self.data.len()
    }

    /// Words for the write to 0x66: `[frame_id, data...]`.
    pub fn payload(&self, frame_id: u32) -> Vec<u32> {
        let mut w = Vec::with_capacity(self.count());
        w.push(frame_id);
        w.extend_from_slice(&self.data);
        w
    }
}

/// Packs records into frames with the vendor flush rule.
#[derive(Clone, Debug, Default)]
pub struct FramePacker {
    data: Vec<u32>,
    items: usize,
    frames: Vec<PackedFrame>,
}

impl FramePacker {
    /// New empty packer.
    pub fn new() -> FramePacker {
        FramePacker::default()
    }

    /// Add one record (one or more items). The frame closes after the record if it holds at least
    /// [`FLUSH_WORDS`] words including the prefix.
    pub fn add_record(&mut self, record: &[FifoItem]) {
        for item in record {
            self.data.extend(item.words());
            self.items += 1;
        }
        if PREFIX_WORDS + self.data.len() >= FLUSH_WORDS {
            self.close();
        }
    }

    /// Data words of the open frame.
    pub fn pending_words(&self) -> usize {
        self.data.len()
    }

    fn close(&mut self) {
        let data = std::mem::take(&mut self.data);
        let items = std::mem::take(&mut self.items);
        self.frames.push(PackedFrame { data, items });
    }

    /// End of the batch: close a non-empty frame and return all frames.
    pub fn finish(mut self) -> Vec<PackedFrame> {
        if !self.data.is_empty() {
            self.close();
        }
        self.frames
    }
}

/// Outcome of one [`FifoFeeder::fill`] pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillResult {
    /// Frames to send, in order, with their ids.
    pub frames: Vec<(u32, PackedFrame)>,
    /// Space left after the pass.
    pub space_left: u32,
    /// Why the pass stopped.
    pub stopped_by: StopReason,
}

/// Why a fill pass stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The ring is drained.
    Empty,
    /// The register 1016 test failed.
    Space,
    /// 50 frames in this pass.
    Limit,
}

/// `fillFifo` flow control over a ring of packed frames.
#[derive(Clone, Debug, Default)]
pub struct FifoFeeder {
    ring: VecDeque<PackedFrame>,
    /// Set when the job's last batch has been queued.
    pub last_batch_pushed: bool,
    /// Frames handed out so far.
    pub frames_sent: u64,
}

impl FifoFeeder {
    /// New feeder with these frames queued.
    pub fn new(frames: Vec<PackedFrame>) -> FifoFeeder {
        FifoFeeder { ring: frames.into(), last_batch_pushed: false, frames_sent: 0 }
    }

    /// Queue more frames; `last` marks the job's final batch.
    pub fn push(&mut self, frames: Vec<PackedFrame>, last: bool) {
        self.ring.extend(frames);
        if last {
            self.last_batch_pushed = true;
        }
    }

    /// Frames not yet handed out.
    pub fn pending(&self) -> usize {
        self.ring.len()
    }

    /// One pass. `frame_id_reg` is register 1015 and `space_margin` register 1016 from the latest
    /// status poll. The frame id of the first frame is `frame_id_reg + 1`, then +1 per frame. The
    /// caller sends each returned frame; if a send fails the caller must stop the job
    /// (`0x67 <- [3]`) rather than skip the frame, because a hole in the stream is a hole in the
    /// toolpath (the vendor would drop it silently).
    pub fn fill(&mut self, frame_id_reg: u32, space_margin: u32, max_frames: usize) -> FillResult {
        let mut frames = Vec::new();
        let mut frame_id = frame_id_reg.wrapping_add(1);
        let mut space = space_margin;
        let stopped_by = loop {
            let Some(f) = self.ring.front() else { break StopReason::Empty };
            if frames.len() >= max_frames {
                break StopReason::Limit;
            }
            let size = f.byte_size();
            if size + FIFO_SEND_HEADROOM_BYTES > space {
                break StopReason::Space;
            }
            let f = self.ring.pop_front().expect("front exists");
            space -= size;
            frames.push((frame_id, f));
            frame_id = frame_id.wrapping_add(1);
            self.frames_sent += 1;
        };
        FillResult { frames, space_left: space, stopped_by }
    }

    /// Job end: last batch pushed, ring empty, register 1016 at 60000 and the program still
    /// reported as running. The caller then sends `0x67 <- [3]`.
    pub fn job_finished(&self, space_margin: u32, processing_status: u32) -> bool {
        self.last_batch_pushed && self.ring.is_empty() && space_margin == FIFO_MARGIN_EMPTY && processing_status & 0xFF == 1
    }
}

/// Bytes the card still holds, from register 1016 (unit unconfirmed).
pub fn queued_bytes(space_margin: u32) -> u32 {
    FIFO_MARGIN_EMPTY.saturating_sub(space_margin)
}

/// Queued 12-byte ticks estimated from register 1016.
pub fn queued_ticks(space_margin: u32) -> u32 {
    queued_bytes(space_margin) / 12
}

/// Text form of a frame in the vendor log style: `40 66 <count> <id> <words...>` in hex.
pub fn format_frame_line(frame_id: u32, frame: &PackedFrame) -> String {
    let mut s = format!("40 66 {:02x} {:02x}", frame.count(), frame_id);
    for w in &frame.data {
        s.push_str(&format!(" {w:02x}"));
    }
    s
}

/// Parse a frame line (plain or containing `DataEx:`) into `(frame_id, data words)`.
pub fn parse_frame_line(line: &str) -> Result<Option<(u32, Vec<u32>)>, FifoParseError> {
    let mut text = line.trim();
    if text.is_empty() || text.starts_with('#') {
        return Ok(None);
    }
    if let Some((_, rest)) = text.split_once("DataEx:") {
        text = rest.trim();
    }
    let mut words = Vec::new();
    for tok in text.split_whitespace() {
        match u32::from_str_radix(tok, 16) {
            Ok(w) => words.push(w),
            Err(_) => return Ok(None),
        }
    }
    if words.len() < 4 || words[0] != 0x40 || words[1] != 0x66 {
        return Ok(None);
    }
    if words[2] as usize != words.len() - 3 {
        return Err(FifoParseError(format!("count word {:#x} != {} words", words[2], words.len() - 3)));
    }
    Ok(Some((words[3], words[4..].to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_encoding_matches_the_documented_header() {
        let t = FifoItem::tick(-3, 7);
        let w = t.words();
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], 0x0008_0BB8);
        assert_eq!(w[1], (7 << 16) | 0xFFFD);
        assert_eq!(w[2], 5000 << 16);
        let d = t.as_tick().unwrap();
        assert_eq!((d.dx, d.dy, d.freq, d.duty), (-3, 7, 5000, 0));
        assert!(!t.is_laser());
        assert_eq!(FifoItem::boundary().words(), vec![0x0000_0BB9]);
        assert_eq!(FifoItem::mode(5).words(), vec![0x0004_0BBA, 5]);
    }

    #[test]
    fn ninety_nine_ticks_close_a_frame() {
        let mut p = FramePacker::new();
        for _ in 0..98 {
            p.add_record(&[FifoItem::tick(1, 0)]);
        }
        assert_eq!(p.pending_words(), 294);
        p.add_record(&[FifoItem::tick(1, 0)]);
        assert_eq!(p.pending_words(), 0);
        p.add_record(&[FifoItem::tick(1, 0)]);
        let frames = p.finish();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data.len(), 297);
        assert_eq!(frames[0].count(), 298);
        assert_eq!(frames[0].byte_size(), 1204);
        assert_eq!(frames[0].items, 99);
        assert_eq!(frames[1].items, 1);
    }

    #[test]
    fn parse_round_trip_and_strictness() {
        let items = [FifoItem::boundary(), FifoItem::mode(4), FifoItem::tick(2, -2)];
        let mut words = vec![9];
        for i in &items {
            words.extend(i.words());
        }
        let f = parse_fifo_words(&words).unwrap();
        assert_eq!(f.frame_id, 9);
        assert_eq!(f.items, items);
        assert_eq!(classify_frame(&f), Ok(Policy::Motion));
        // truncated item
        assert!(parse_fifo_words(&words[..words.len() - 1]).is_err());
        // unknown opcode
        assert!(parse_fifo_words(&[1, 0x0000_1234]).is_err());
        // wrong payload length for a known opcode
        assert!(parse_fifo_words(&[1, 0x0004_0BB8, 0]).is_err());
        assert!(parse_fifo_words(&[]).is_err());
    }

    #[test]
    fn laser_content_is_classified_not_hidden() {
        // tick with duty 4
        let w = [1, 0x0008_0BB8, 0, (5000 << 16) | 4];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
        // DO9 on
        let w = [1, 0x000C_270F, 2, 0x100, 0x100];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
        // DO9 off is fine
        let w = [1, 0x000C_270F, 2, 0x100, 0];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Motion));
        // DO3 high air on is a gas record, not laser
        let w = [1, 0x000C_270F, 2, 4, 4];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Motion));
        // PWM set
        let w = [1, 0x000C_270F, 0x11, 1234, 10];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
        // DO on a port outside the allow-list
        let w = [1, 0x000C_270F, 2, 0x8000, 0x8000];
        assert!(classify_frame(&parse_fifo_words(&w).unwrap()).is_err());
        // unknown 9999 sub-record
        let w = [1, 0x000C_270F, 16, 0, 0];
        assert!(classify_frame(&parse_fifo_words(&w).unwrap()).is_err());
        // follower records are in the grammar and decode, but are not motion-level data
        let w = [1, 0x0008_0067, 1000, 0];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
    }

    #[test]
    fn feeder_follows_the_vendor_flow_control() {
        let mut p = FramePacker::new();
        for _ in 0..300 {
            p.add_record(&[FifoItem::tick(1, 1)]);
        }
        let frames = p.finish();
        assert_eq!(frames.len(), 4);
        let mut f = FifoFeeder::new(frames);
        // Plenty of space: all four go out, ids start at reg + 1.
        let r = f.fill(10, 60000, MAX_FRAMES_PER_FILL);
        assert_eq!(r.frames.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![11, 12, 13, 14]);
        assert_eq!(r.stopped_by, StopReason::Empty);
        assert_eq!(f.frames_sent, 4);
        // Space test: 1204 + 2000 > 3000.
        let mut p = FramePacker::new();
        for _ in 0..99 {
            p.add_record(&[FifoItem::tick(1, 1)]);
        }
        let mut f = FifoFeeder::new(p.finish());
        let r = f.fill(0, 3000, MAX_FRAMES_PER_FILL);
        assert!(r.frames.is_empty());
        assert_eq!(r.stopped_by, StopReason::Space);
        let r = f.fill(0, 3204, MAX_FRAMES_PER_FILL);
        assert_eq!(r.frames.len(), 1);
        assert_eq!(r.space_left, 2000);
    }

    #[test]
    fn feeder_limit_and_job_end() {
        let frames: Vec<PackedFrame> = (0..60).map(|_| PackedFrame { data: vec![0x0000_0BB9], items: 1 }).collect();
        let mut f = FifoFeeder::new(frames);
        let r = f.fill(0, 60000, 50);
        assert_eq!(r.frames.len(), 50);
        assert_eq!(r.stopped_by, StopReason::Limit);
        assert!(!f.job_finished(60000, 1));
        f.push(vec![], true);
        let _ = f.fill(50, 60000, 50);
        assert!(f.job_finished(60000, 1));
        assert!(!f.job_finished(59000, 1));
        assert!(!f.job_finished(60000, 0));
    }

    #[test]
    fn frame_lines() {
        let frame = PackedFrame { data: vec![0x0000_0BB9, 0x0004_0BBA, 5], items: 2 };
        let line = format_frame_line(0x3a, &frame);
        assert_eq!(line, "40 66 04 3a bb9 40bba 05");
        assert_eq!(parse_frame_line(&line).unwrap(), Some((0x3a, vec![0xbb9, 0x40bba, 5])));
        assert_eq!(parse_frame_line(&format!("12:00 DataEx:{line}")).unwrap().unwrap().0, 0x3a);
        assert_eq!(parse_frame_line("# comment").unwrap(), None);
        assert_eq!(parse_frame_line("40 65 2 1 2").unwrap(), None);
        assert!(parse_frame_line("40 66 9 3a bb9").is_err());
    }

    #[test]
    fn queue_depth_estimates() {
        assert_eq!(queued_bytes(60000), 0);
        assert_eq!(queued_bytes(70000), 0);
        assert_eq!(queued_ticks(60000 - 120), 10);
    }
}
