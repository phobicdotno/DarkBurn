//! FIFO items, frame packing and flow control for register 0x66.
//!
//! Item grammar: `header = (payload_bytes << 16) | opcode` followed by `payload_bytes / 4` words.
//! The layouts below come from captures of the vendor's own job stream (CO2 grid and circle jobs,
//! fiber frame, dry cut and cut; see `tests/data/mlaser-grid-co2.frames`):
//!
//! * Opcode 3000 is one motion tick of 250 us: `[0x0008_0BB8, (form << 24) | (power << 16) |
//!   (dy as u8) << 8 | (dx as u8), tail]`. `dx` (low byte, card axis 0) and `dy` (next byte, axis 1)
//!   are signed 8-bit step counts (X 257.90, Y 258.14 counts/mm). `power` is the power byte (CO2:
//!   the layer power %, fiber: a per-tick power that ramps). `form` is `0x88` for the CO2 and travel
//!   form with tail `0x13` (laser off) or `0xffff_0013` (laser on), and `0x57` for the fiber form
//!   with tail `0x04` or `0xffff_0004`. Acceleration lives in the deltas; there is no speed word.
//!   On the card's own axes, positive X moves the head right and positive Y moves it towards
//!   the back (seen on an M3 Ultra with jog steps). That a positive FIFO `dx`/`dy` follows the
//!   same axis sign is assumed, not seen yet: check it in a dry run before cutting.
//! * Opcode 3001 is a boundary record without payload.
//! * Opcode 3002 is the segment counter `[n]`: 0, 1, 2 ... over the job, even = travel segment,
//!   odd = laser segment. It is not a mode word.
//! * Opcode 9999 carries 0x65-style records: DO `[2, mask, value]`, DA `[4, channel, value]`,
//!   CO2 PWM `[0x11, freq, duty %, enable]`, fiber laser `[3, freq, power %, enable]`, and the
//!   laser-off marker `[0x12, 0]`.
//!
//! A frame is `WRITE 0x66` with words `[frame_id, items...]`; the packer closes a frame when the
//! vector `[0x40, 0x66, count, frame_id, items...]` reaches 300 words (99 ticks) or at the end of a
//! batch. [`FifoFeeder`] reproduces the vendor flow control: frame id is register 1015 plus one, a
//! frame is sent only if `frame_bytes + 2000 <= register 1016`, at most 50 frames per pass.
//!
//! Public items can only be built as *dry* items (power 0, laser-off ticks, boundary and segment
//! records). The full grammar can be decoded for the simulator, but a frame carrying laser data or
//! any out-of-grammar item is refused by [`crate::command::SafeWrite`].

use crate::command::{Policy, SafetyConfig};
use crate::registers::{FIFO_MARGIN_EMPTY, FIFO_SEND_HEADROOM_BYTES, Forbidden};
use std::collections::VecDeque;

/// Tick opcode.
pub const OP_TICK: u16 = 3000;
/// Boundary record, inserted when a control record follows a tick.
pub const OP_BOUNDARY: u16 = 3001;
/// Segment counter `[n]`: rises by one per segment, even = travel, odd = laser.
pub const OP_SEGMENT: u16 = 3002;
/// In-stream 0x65-style record (DO, DA, PWM, laser).
pub const OP_MISC: u16 = 9999;
/// Tick period in microseconds.
pub const TICK_US: u32 = 250;
/// Steps per millimetre on card axis 0 (X), from a 250 mm capture line.
pub const COUNTS_PER_MM_X: f64 = 257.90;
/// Steps per millimetre on card axis 1 (Y), from a 250 mm capture line.
pub const COUNTS_PER_MM_Y: f64 = 258.14;
/// Largest step a tick carries on one axis.
pub const TICK_MAX_STEP: i32 = i8::MAX as i32;
/// High half of a tick tail with the beam on.
pub const TICK_TAIL_LASER_ON: u32 = 0xFFFF_0000;
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
        9999 => &[2, 3, 4],
        103 | 109 => &[2],
        104 => &[2, 8],
        105 | 108 => &[4],
        106 => &[6],
        118 => &[3],
        2001 => &[2],
        _ => return None,
    })
}

/// Which head a tick is written for: the top byte of the tick word and the low half of its tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TickForm {
    /// `0x88`, tail `0x13`: CO2 cuts and every travel or frame move.
    Co2,
    /// `0x57`, tail `0x04`: fiber cuts and fiber dry cuts.
    Fiber,
}

impl TickForm {
    /// Top byte of the tick word.
    pub const fn code(self) -> u8 {
        match self {
            TickForm::Co2 => 0x88,
            TickForm::Fiber => 0x57,
        }
    }

    /// Low half of the tail word.
    pub const fn tail(self) -> u32 {
        match self {
            TickForm::Co2 => 0x13,
            TickForm::Fiber => 0x04,
        }
    }

    fn from_code(code: u8) -> Option<TickForm> {
        match code {
            0x88 => Some(TickForm::Co2),
            0x57 => Some(TickForm::Fiber),
            _ => None,
        }
    }
}

/// One decoded 3000 tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tick {
    /// Tick form (head).
    pub form: TickForm,
    /// Power byte. Always 0 for items this crate builds.
    pub power: u8,
    /// Steps on card axis 0 (X) in this tick.
    pub dx: i8,
    /// Steps on card axis 1 (Y) in this tick.
    pub dy: i8,
    /// Tail says beam on (`0xffff` high half). Always false for items this crate builds.
    pub laser_on: bool,
}

impl Tick {
    /// The two payload words `[form|power|dy|dx, tail]`.
    pub fn encode(&self) -> [u32; 2] {
        let word = (u32::from(self.form.code()) << 24) | (u32::from(self.power) << 16) | (u32::from(self.dy as u8) << 8) | u32::from(self.dx as u8);
        let tail = self.form.tail() | if self.laser_on { TICK_TAIL_LASER_ON } else { 0 };
        [word, tail]
    }

    /// Decode the two payload words; `None` for a form or tail never seen from the vendor.
    pub fn decode(word: u32, tail: u32) -> Option<Tick> {
        let form = TickForm::from_code((word >> 24) as u8)?;
        let laser_on = match tail & 0xFFFF_0000 {
            0 => false,
            TICK_TAIL_LASER_ON => true,
            _ => return None,
        };
        if tail & 0xFFFF != form.tail() {
            return None;
        }
        Some(Tick { form, power: (word >> 16) as u8, dx: word as u8 as i8, dy: (word >> 8) as u8 as i8, laser_on })
    }

    /// True if the tick turns the beam on or carries power.
    pub fn is_laser(&self) -> bool {
        self.laser_on || self.power != 0
    }
}

/// One FIFO item. The fields are private: the only public constructors build dry items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoItem {
    opcode: u16,
    payload: Vec<u32>,
}

impl FifoItem {
    /// A dry motion tick in the CO2/travel form: moves `dx`, `dy` steps with power 0, beam off.
    pub fn tick(dx: i8, dy: i8) -> FifoItem {
        FifoItem::tick_form(TickForm::Co2, dx, dy)
    }

    /// A dry motion tick in `form`: power 0, beam off.
    pub fn tick_form(form: TickForm, dx: i8, dy: i8) -> FifoItem {
        let t = Tick { form, power: 0, dx, dy, laser_on: false };
        FifoItem { opcode: OP_TICK, payload: t.encode().to_vec() }
    }

    /// A boundary record.
    pub fn boundary() -> FifoItem {
        FifoItem { opcode: OP_BOUNDARY, payload: vec![] }
    }

    /// A segment counter record. Inside one frame the counter must rise by one per record.
    pub fn segment(n: u32) -> FifoItem {
        FifoItem { opcode: OP_SEGMENT, payload: vec![n] }
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

    /// The tick view of a 3000 item; `None` for other items and for ticks in an unknown form.
    pub fn as_tick(&self) -> Option<Tick> {
        match (self.opcode, &self.payload[..]) {
            (OP_TICK, [word, tail]) => Tick::decode(*word, *tail),
            _ => None,
        }
    }

    /// The counter of a 3002 segment record.
    pub fn as_segment(&self) -> Option<u32> {
        match (self.opcode, &self.payload[..]) {
            (OP_SEGMENT, [n]) => Some(*n),
            _ => None,
        }
    }

    /// True if the item carries laser data. Such an item is decodable (so the simulator can model
    /// the vendor gate) but no public constructor creates one and [`crate::command::SafeWrite`]
    /// refuses frames containing one.
    pub fn is_laser(&self) -> bool {
        self.is_laser_with(LASER_DO_MASK)
    }

    /// [`is_laser`](Self::is_laser) judged by the laser ports of `cfg` instead of the default ones.
    pub fn is_laser_for(&self, cfg: &SafetyConfig) -> bool {
        self.is_laser_with(SafetyConfig::port_mask(&cfg.laser_do_ports))
    }

    fn is_laser_with(&self, laser_do_mask: u32) -> bool {
        match self.opcode {
            // A tick in an unknown form counts as laser data: nothing proves its tail is off.
            OP_TICK => self.as_tick().is_none_or(|t| t.is_laser()),
            OP_MISC => match self.payload.first() {
                // DO: any laser port bit switched on (by default 5 fibre gate, 6 pointer, 9 CO2).
                Some(2) => self.payload.get(2).is_some_and(|v| v & laser_do_mask != 0),
                // Fiber laser command and CO2 PWM: always laser data in a stream.
                Some(3) | Some(0x11) => true,
                // DA: non-zero value.
                Some(4) => self.payload.get(2).is_some_and(|v| *v != 0),
                // Laser-off marker: laser data, so the gate and the frame classifier agree.
                Some(0x12) => true,
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
    classify_frame_with(frame, LASER_DO_MASK, ALLOWED_DO_MASK)
}

/// [`classify_frame`] with the laser, lamp and gas ports of `cfg` instead of the default ones.
pub fn classify_frame_for(frame: &FifoFrame, cfg: &SafetyConfig) -> Result<Policy, Forbidden> {
    let laser = SafetyConfig::port_mask(&cfg.laser_do_ports);
    classify_frame_with(frame, laser, laser | SafetyConfig::port_mask(&cfg.motion_do_ports))
}

fn classify_frame_with(frame: &FifoFrame, laser_do_mask: u32, allowed_do_mask: u32) -> Result<Policy, Forbidden> {
    let mut laser = false;
    let mut last_segment: Option<u32> = None;
    for item in &frame.items {
        let p = &item.payload[..];
        match item.opcode {
            OP_TICK if item.as_tick().is_none() => {
                return Err(Forbidden::FifoFrame(format!("tick {p:x?} is not in the 0x88/0x13 or 0x57/0x04 form")));
            }
            OP_SEGMENT => {
                // The counter rises by one per segment over the whole job; inside one frame a
                // second record must follow the first directly.
                let n = p[0];
                if let Some(prev) = last_segment
                    && prev.checked_add(1) != Some(n)
                {
                    return Err(Forbidden::FifoFrame(format!("segment counter goes from {prev} to {n} inside one frame")));
                }
                last_segment = Some(n);
            }
            OP_MISC => match p {
                [2, mask, value] => {
                    if (mask | value) & !allowed_do_mask != 0 {
                        return Err(Forbidden::FifoFrame(format!("DO record {p:?} addresses a port outside the allow-list")));
                    }
                }
                [4, _, _] | [3 | 0x11, _, _, _] => {}
                [13, _, value] => {
                    if *value != 0 {
                        return Err(Forbidden::FifoFrame("extended DO on".into()));
                    }
                }
                // Laser-off marker of the vendor's laser-off block: copied, never built.
                [0x12, 0] => laser = true,
                _ => return Err(Forbidden::FifoFrame(format!("9999 record {p:?} not allowed"))),
            },
            _ => {}
        }
        // Follower moves, follow and pierce records (103..=109, 118) and 2001 belong to the
        // vendor's height-follower and pierce logic. Like laser data they decode, so a simulator
        // can model them, but they take the level no public constructor reaches.
        let vendor_only = !matches!(item.opcode, OP_TICK | OP_BOUNDARY | OP_SEGMENT | OP_MISC);
        if item.is_laser_with(laser_do_mask) || vendor_only {
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
    fn tick_encoding_matches_the_capture() {
        let t = FifoItem::tick(-3, 7);
        let w = t.words();
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], 0x0008_0BB8);
        assert_eq!(w[1], 0x8800_07FD);
        assert_eq!(w[2], 0x13);
        let d = t.as_tick().unwrap();
        assert_eq!((d.form, d.power, d.dx, d.dy, d.laser_on), (TickForm::Co2, 0, -3, 7, false));
        assert!(!t.is_laser());
        // one-count ticks at the start of a cut, from the GAP notes
        assert_eq!(Tick::decode(0x8819_0100, 0xFFFF_0013), Some(Tick { form: TickForm::Co2, power: 0x19, dx: 0, dy: 1, laser_on: true }));
        assert_eq!(Tick::decode(0x8819_0001, 0xFFFF_0013).map(|t| (t.dx, t.dy)), Some((1, 0)));
        // fiber form from the dry cut and the cut
        assert_eq!(FifoItem::tick_form(TickForm::Fiber, -1, 2).words(), vec![0x0008_0BB8, 0x5700_02FF, 0x04]);
        let f = Tick::decode(0x5716_02FF, 0xFFFF_0004).unwrap();
        assert_eq!((f.form, f.power, f.dx, f.dy, f.laser_on), (TickForm::Fiber, 0x16, -1, 2, true));
        assert_eq!(f.encode(), [0x5716_02FF, 0xFFFF_0004]);
        // forms and tails never seen
        assert_eq!(Tick::decode(0x8800_0000, 0x04), None, "CO2 word with the fiber tail");
        assert_eq!(Tick::decode(0x5700_0000, 0x13), None, "fiber word with the CO2 tail");
        assert_eq!(Tick::decode(0x0000_0001, 0x13), None, "old (dY<<16)|dX layout");
        assert_eq!(Tick::decode(0x8800_0000, 0x1388_0013), None);
        assert_eq!(FifoItem::boundary().words(), vec![0x0000_0BB9]);
        assert_eq!(FifoItem::segment(5).words(), vec![0x0004_0BBA, 5]);
        assert_eq!(FifoItem::segment(0x61).as_segment(), Some(0x61));
    }

    #[test]
    fn extreme_steps_round_trip() {
        for (dx, dy) in [(i8::MIN, i8::MAX), (i8::MAX, i8::MIN), (-1, -1), (0, 0)] {
            for form in [TickForm::Co2, TickForm::Fiber] {
                let t = FifoItem::tick_form(form, dx, dy).as_tick().unwrap();
                assert_eq!((t.form, t.dx, t.dy, t.power, t.laser_on), (form, dx, dy, 0, false));
            }
        }
    }

    /// Real blocks from a Mlaser CO2 grid job (`tests/data/mlaser-grid-co2.frames`).
    fn capture_blocks() -> Vec<(u32, Vec<u32>)> {
        include_str!("../tests/data/mlaser-grid-co2.frames").lines().filter_map(|l| parse_frame_line(l).unwrap()).collect()
    }

    #[test]
    fn real_mlaser_blocks_parse_and_classify() {
        let blocks = capture_blocks();
        assert_eq!(blocks.iter().map(|b| b.0).collect::<Vec<_>>(), vec![0x474, 0x479, 0x4FA, 0x511]);
        let mut policies = Vec::new();
        let mut segments = Vec::new();
        for (id, data) in &blocks {
            let mut words = vec![*id];
            words.extend(data);
            let f = parse_fifo_words(&words).unwrap_or_else(|e| panic!("block {id:#x}: {e}"));
            for item in &f.items {
                if item.opcode() == OP_TICK {
                    assert!(item.as_tick().is_some(), "block {id:#x}: tick {:x?}", item.payload());
                }
                segments.extend(item.as_segment());
            }
            policies.push(classify_frame(&f).unwrap_or_else(|e| panic!("block {id:#x}: {e:?}")));
        }
        // travel-only block is motion; the laser-on and laser-off blocks are laser level
        assert_eq!(policies, vec![Policy::Motion, Policy::Laser, Policy::Laser, Policy::Motion]);
        assert_eq!(segments, vec![0, 1, 2]);
        let items = |b: &(u32, Vec<u32>)| {
            let mut words = vec![b.0];
            words.extend(&b.1);
            parse_fifo_words(&words).unwrap().items
        };
        // the laser segment starts with the 25 % PWM [0x11, 5000 Hz, 25 %, enable], then beam-on ticks
        let b1 = items(&blocks[1]);
        let pwm = b1.iter().position(|i| i.payload() == [0x11, 5000, 0x19, 1]).expect("PWM record");
        assert!(b1[pwm + 1..].iter().all(|i| i.as_tick().is_some_and(|t| t.laser_on)));
        // beam-on ticks carry power byte 0x19, then the laser-off block and travel again
        let on: Vec<Tick> = b1.iter().chain(&items(&blocks[2])).filter_map(FifoItem::as_tick).filter(|t| t.laser_on).collect();
        assert!(on.len() > 100 && on.iter().all(|t| t.power == 0x19 && t.form == TickForm::Co2));
        let b2 = items(&blocks[2]);
        let off = b2.iter().position(|i| i.payload() == [0x12, 0]).expect("laser-off marker");
        assert!(b2[off..].iter().filter_map(FifoItem::as_tick).all(|t| !t.is_laser()));
    }

    #[test]
    fn real_travel_block_round_trips_through_the_encoder() {
        // Rebuild the first block of the job from decoded values with the public dry
        // constructors; the words must equal what Mlaser sent.
        let (id, data) = &capture_blocks()[0];
        let mut words = vec![*id];
        words.extend(data);
        let f = parse_fifo_words(&words).unwrap();
        let mut rebuilt = Vec::new();
        for item in &f.items {
            let again = match (item.as_segment(), item.as_tick()) {
                (Some(n), _) => FifoItem::segment(n),
                (_, Some(t)) => {
                    assert!(!t.is_laser());
                    FifoItem::tick_form(t.form, t.dx, t.dy)
                }
                _ => panic!("unexpected item {item:?}"),
            };
            rebuilt.push(again);
        }
        let mut p = FramePacker::new();
        p.add_record(&rebuilt);
        let frames = p.finish();
        assert_eq!(frames.len(), 1);
        assert_eq!(&frames[0].data, data);
        let w = crate::command::fifo_packed_frame(*id, &frames[0]).unwrap();
        assert_eq!(w.policy(), Policy::Motion);
        // every travel tick in the capture also re-encodes word for word
        for (_, data) in capture_blocks() {
            let mut words = vec![0];
            words.extend(&data);
            for item in parse_fifo_words(&words).unwrap().items {
                if let Some(t) = item.as_tick() {
                    assert_eq!(t.encode().to_vec(), item.payload());
                }
            }
        }
    }

    #[test]
    fn fiber_laser_records_from_the_capture() {
        // Fiber contour head and tail as Mlaser sent them: laser command [3, freq, power %, on],
        // beam-on 0x57 ticks, laser-off marker and laser command off.
        #[rustfmt::skip]
        let words = [
            1, 0x0004_0BBA, 3, 0x0000_0BB9, 0x000C_270F, 2, 2, 2, 0x0010_270F, 3, 0x457, 0x5F, 1, 0x0008_0BB8, 0x5701_0100,
            0xFFFF_0004, 0x0000_0BB9, 0x0008_270F, 0x12, 0, 0x0010_270F, 3, 0x457, 0, 0, 0x000C_270F, 2, 2, 0, 0x0000_0BB9,
            0x0004_0BBA, 4,
        ];
        let f = parse_fifo_words(&words).unwrap();
        assert_eq!(f.items.len(), 11);
        assert_eq!(classify_frame(&f), Ok(Policy::Laser));
        // the dry cut: fiber ticks with the beam off, laser command at zero
        let dry = [1, 0x0008_0BB8, 0x5700_0100, 4, 0x0008_0BB8, 0x5700_02FF, 4];
        assert_eq!(classify_frame(&parse_fifo_words(&dry).unwrap()), Ok(Policy::Motion));
    }

    #[test]
    fn segment_counter_rises_inside_a_frame() {
        let ok = [1, 0x0004_0BBA, 0x60, 0x0000_0BB9, 0x0004_0BBA, 0x61];
        assert_eq!(classify_frame(&parse_fifo_words(&ok).unwrap()), Ok(Policy::Motion));
        // any start value: the counter runs over the whole job
        for n in [0, 1, 0x61, 3001] {
            assert_eq!(classify_frame(&parse_fifo_words(&[1, 0x0004_0BBA, n]).unwrap()), Ok(Policy::Motion), "{n}");
        }
        for (a, b) in [(3, 3), (4, 3), (1, 5), (u32::MAX, 0)] {
            let w = [1, 0x0004_0BBA, a, 0x0004_0BBA, b];
            assert!(classify_frame(&parse_fifo_words(&w).unwrap()).is_err(), "{a} -> {b}");
        }
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
        let items = [FifoItem::boundary(), FifoItem::segment(4), FifoItem::tick(2, -2)];
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
        // a tick in the old (dY<<16)|dX, (freq<<16)|duty layout parses but is refused
        let old = parse_fifo_words(&[1, 0x0008_0BB8, (7 << 16) | 0xFFFD, 5000 << 16]).unwrap();
        assert!(classify_frame(&old).is_err());
        assert!(old.items[0].is_laser(), "an unknown tick form is never taken as dry");
    }

    #[test]
    fn laser_content_is_classified_not_hidden() {
        // CO2 tick with the beam on
        let w = [1, 0x0008_0BB8, 0x8819_0001, 0xFFFF_0013];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
        // power byte without the beam-on tail is still laser data
        let w = [1, 0x0008_0BB8, 0x8819_0001, 0x13];
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
        // PWM set [0x11, freq, duty, enable]
        let w = [1, 0x0010_270F, 0x11, 5000, 25, 1];
        assert_eq!(classify_frame(&parse_fifo_words(&w).unwrap()), Ok(Policy::Laser));
        // a DO or PWM record of the wrong length
        assert!(classify_frame(&parse_fifo_words(&[1, 0x0008_270F, 2, 0x100]).unwrap()).is_err());
        assert!(classify_frame(&parse_fifo_words(&[1, 0x000C_270F, 0x11, 1234, 10]).unwrap()).is_err());
        // laser-off marker decodes at the laser level only
        assert_eq!(classify_frame(&parse_fifo_words(&[1, 0x0008_270F, 0x12, 0]).unwrap()), Ok(Policy::Laser));
        assert!(classify_frame(&parse_fifo_words(&[1, 0x0008_270F, 0x12, 1]).unwrap()).is_err());
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
