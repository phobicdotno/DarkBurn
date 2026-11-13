//! Dissector: says in plain words what a datagram or a request vector is.
//!
//! Decode only. It names every operation the owner's analysis found, including the ones this
//! crate refuses to build (follower moves, licence registers, firmware download), so that a
//! read-only capture from the real card can be read and checked against the analysis. Nothing in
//! this module can encode a frame, and [`Verdict`] says for each request whether the crate could.
//!
//! Distances and speeds are shown with K = 1000 (micrometres); the card's own K is register 50017.

use crate::command::{Policy, SafeWrite, SafetyConfig};
use crate::frame::{EXCEPTION_BIT, FUNC_BYTE_BLOCK, FUNC_READ, FUNC_STREAM, FUNC_STREAM_ALT, FUNC_WRITE, RawFrame, exception_errcode, func_name};
use crate::registers::{BLOCKS, Forbidden, HARD_PARAM_BASE, LICENCE_FIRST, LICENCE_LAST, REG_COMMAND, REG_FIFO_CONTROL, REG_FIFO_DATA, check_read};

/// K used for display.
pub const DISPLAY_K: f64 = 1000.0;

/// Name of a 0x65 sub-command (first word of the vector). A trailing `?` marks a name the
/// analysis is not sure of.
pub fn command_name(sub: u32) -> &'static str {
    match sub {
        1 => "stop",
        2 => "home",
        3 => "jog",
        4 => "sub4?",
        5 => "go-to",
        7 => "sub7?",
        101 => "follower stop",
        102 => "follower resync?",
        103 => "follower move",
        104 => "follower 104",
        107 => "follower calibrate?",
        109 => "follower lift",
        117 => "follower pierce?",
        118 => "follower bookkeeping",
        9999 => "misc",
        _ => "unknown",
    }
}

/// Name of a 9999 sub-command (second word of the vector).
pub fn misc_name(sub: u32) -> &'static str {
    match sub {
        1 => "queue clear (old form)",
        2 => "outputs 1..10",
        3 => "PWM",
        4 => "analogue output",
        5 => "connect prologue",
        13 => "outputs 11..26",
        16 => "per-record?",
        17 => "PWM (5 V)",
        _ => "unknown",
    }
}

/// Name of a FIFO item opcode.
pub fn fifo_opcode_name(op: u16) -> &'static str {
    match op {
        3000 => "tick",
        3001 => "boundary",
        3002 => "mode",
        9999 => "misc",
        103 => "follower move",
        104 => "follower 104",
        105 => "follower follow",
        106 => "follower section pierce",
        108 => "follower gradual pierce",
        109 => "follower lift",
        118 => "follower bookkeeping",
        2001 => "wait",
        2002 => "fibre 2002",
        2004 => "fibre 2004",
        3 => "fibre 3",
        _ => "unknown",
    }
}

/// Name of a 0x67 control value.
pub fn fifo_control_name(value: u32) -> &'static str {
    match value {
        1 => "clear",
        2 => "start",
        3 => "stop",
        _ => "unknown",
    }
}

/// What a register address is, as far as the analysis knows.
pub fn register_name(addr: u32) -> String {
    match addr {
        REG_COMMAND => "command".into(),
        REG_FIFO_DATA => "motion queue data".into(),
        REG_FIFO_CONTROL => "motion queue control".into(),
        100 => "restart / apply (meaning unproven)".into(),
        150 => "follower factory reset / commit".into(),
        151 => "edge-seek records".into(),
        200 | 201 => "test-data window".into(),
        LICENCE_FIRST..=LICENCE_LAST => "licence / clock area".into(),
        _ => {
            if let Some(b) = BLOCKS.iter().find(|b| b.start <= addr && addr < b.start + b.count.max(1)) {
                return if addr == b.start { b.name.to_string() } else { format!("{} + {}", b.name, addr - b.start) };
            }
            match addr {
                12002..=12801 => "follower calibration table".into(),
                13000..=13399 => "auto-focus".into(),
                a if a >= HARD_PARAM_BASE => "hardware parameters".into(),
                _ => "unknown".into(),
            }
        }
    }
}

fn s32(w: u32) -> i64 {
    w as i32 as i64
}

fn mm(w: u32) -> String {
    format!("{:+.3} mm", s32(w) as f64 / DISPLAY_K)
}

fn mm_s(w: u32) -> String {
    format!("{:.3} mm/s", s32(w) as f64 / DISPLAY_K)
}

fn axis_name(slot: u32) -> String {
    match slot {
        0 => "X".into(),
        1 => "Y".into(),
        2 => "Y2".into(),
        3 => "Z".into(),
        4 => "W".into(),
        other => format!("bit{other}"),
    }
}

fn mask_names(mask: u32) -> String {
    if mask == 0 {
        return "none".into();
    }
    (0..32).filter(|b| mask & (1 << b) != 0).map(axis_name).collect::<Vec<_>>().join("|")
}

/// Meaning of the words after `9999`.
pub fn describe_misc(w: &[u32]) -> String {
    match w {
        [sub @ (2 | 13), mask, value, ..] => {
            let base = if *sub == 2 { 1 } else { 11 };
            let ports: Vec<String> =
                (0..32).filter(|b| mask & (1 << b) != 0).map(|b| format!("DO{}={}", base + b, u32::from(value & (1 << b) != 0))).collect();
            if ports.is_empty() { format!("output mask {mask:#x} value {value:#x}") } else { format!("set {}", ports.join(" ")) }
        }
        [4, channel, millivolts, ..] => format!("analogue output {} = {millivolts} mV", channel.wrapping_add(1)),
        [3 | 0x11, freq, duty, ..] => format!("PWM {freq} Hz, duty {duty}"),
        [5, ..] => "connect prologue (meaning unknown)".into(),
        _ => String::new(),
    }
}

/// Meaning of a `WRITE 0x65` word list. Empty when the sub-command has no known word layout.
pub fn describe_command(data: &[u32]) -> String {
    let Some((&sub, w)) = data.split_first() else { return String::new() };
    match (sub, w) {
        (1, [mask, word2, vd, jerk, ..]) => format!("stop axes {}, decel {vd} (jerk {jerk}), word2={word2}", mask_names(*mask)),
        (1, [mask]) => format!("stop axes {}, card default decel", mask_names(*mask)),
        (2, [mask, ..]) => {
            let kind = if mask.is_power_of_two() { "home" } else { "system home" };
            format!("{kind} axes {}", mask_names(*mask))
        }
        (3, [axis, v, a, jerk, d, ..]) => {
            let (idx, absolute) = (axis & 0x7FFF_FFFF, axis & 0x8000_0000 != 0);
            let target = if absolute { format!("target {}", mm(*d)) } else { format!("distance {} (relative)", mm(*d)) };
            format!("axis {} {target}, v {}, a {a} mm/s^2, jerk {jerk}", axis_name(idx), mm_s(*v))
        }
        (5, [mask, v, a, jerk, x, y, c, d, ..]) => {
            let (m, absolute) = (mask & 0x7FFF_FFFF, mask & 0x8000_0000 != 0);
            format!(
                "{} move of {} to X {}, Y {}, W {}, Z {}, v {}, a {a} mm/s^2, jerk {jerk}",
                if absolute { "absolute" } else { "relative" },
                mask_names(m),
                mm(*x),
                mm(*y),
                mm(*c),
                mm(*d),
                mm_s(*v)
            )
        }
        (9999, [_, ..]) => describe_misc(w),
        (103 | 104 | 109, [v, h, ..]) => format!("follower v {:.1} mm/s, height {:+.3} mm", *v as f64 / 10.0, s32(*h) as f64 / 1000.0),
        _ => String::new(),
    }
}

/// One line for the data words of a `WRITE 0x66`: frame id, item count and a census by opcode.
/// A frame that does not split into whole items says so instead.
pub fn describe_fifo(words: &[u32]) -> String {
    let Some((&frame_id, rest)) = words.split_first() else { return "queue frame (empty: no frame id)".into() };
    let mut census: Vec<(u16, usize)> = Vec::new();
    let (mut i, mut items, mut ticks_with_duty) = (0usize, 0usize, 0usize);
    let mut remainder = 0usize;
    while i < rest.len() {
        let (bytes, op) = ((rest[i] >> 16) as usize, (rest[i] & 0xFFFF) as u16);
        let n = bytes / 4;
        if !bytes.is_multiple_of(4) || i + 1 + n > rest.len() {
            remainder = rest.len() - i;
            break;
        }
        if op == 3000 && n == 2 && rest[i + 2] & 0xFFFF != 0 {
            ticks_with_duty += 1;
        }
        match census.iter_mut().find(|(o, _)| *o == op) {
            Some(entry) => entry.1 += 1,
            None => census.push((op, 1)),
        }
        items += 1;
        i += 1 + n;
    }
    census.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let parts: Vec<String> = census.iter().map(|(op, n)| format!("{} x{n}", fifo_opcode_name(*op))).collect();
    let mut out = format!("queue frame id={frame_id} ({frame_id:#x}), {items} items [{}]", parts.join(", "));
    if ticks_with_duty > 0 {
        out.push_str(&format!(", {ticks_with_duty} ticks with duty"));
    }
    if remainder > 0 {
        out.push_str(&format!(", {remainder} words left over"));
    }
    out
}

/// One line for a request vector `[func, addr, count, data...]` as the vendor logs it.
pub fn describe_vector(vector: &[u32]) -> String {
    let Some(&func) = vector.first() else { return "(empty)".into() };
    let known = [FUNC_READ, FUNC_WRITE, FUNC_BYTE_BLOCK, FUNC_STREAM, FUNC_STREAM_ALT].iter().any(|&f| f as u32 == func);
    if !known {
        if func & EXCEPTION_BIT as u32 != 0 && vector.len() == 2 {
            return format!("EXCEPTION {:#04x} code {}", func & 0x7F, vector[1]);
        }
        let rest: Vec<String> = vector[1..].iter().map(|w| format!("{w:x}")).collect();
        return format!("func {func:#04x} {}", rest.join(" ")).trim_end().to_string();
    }
    let func = func as u8;
    if func == FUNC_STREAM || func == FUNC_STREAM_ALT {
        return format!("{} {} bytes", func_name(func), vector.get(1).copied().unwrap_or(0));
    }
    let [_, addr, count, data @ ..] = vector else { return format!("{} (short vector)", func_name(func)) };
    match func {
        FUNC_READ => format!("READ {addr} x{count} ({})", register_name(*addr)),
        FUNC_BYTE_BLOCK => format!("BYTEBLOCK {addr} x{count} (firmware or file download)"),
        _ => match *addr {
            REG_FIFO_DATA => describe_fifo(data),
            REG_FIFO_CONTROL if data.len() == 1 => format!("queue control {} ({})", data[0], fifo_control_name(data[0])),
            REG_COMMAND if !data.is_empty() => {
                let sub = data[0];
                let name = if sub == 9999 && data.len() > 1 { format!("misc/{}", misc_name(data[1])) } else { command_name(sub).to_string() };
                // The axis word of sub-commands 1, 2, 3 and 5 is a bit field (bit 31 = absolute),
                // so it is shown unsigned in hex.
                let shown: Vec<String> = data[1..]
                    .iter()
                    .enumerate()
                    .map(|(i, w)| if i == 0 && matches!(sub, 1 | 2 | 3 | 5) { format!("{w:#x}") } else { s32(*w).to_string() })
                    .collect();
                let meaning = describe_command(data);
                let mut out = format!("CMD {name} [{sub}{}{}]", if shown.is_empty() { "" } else { ", " }, shown.join(", "));
                if !meaning.is_empty() {
                    out.push_str("  ");
                    out.push_str(&meaning);
                }
                out
            }
            _ => {
                let shown: Vec<String> = data.iter().take(8).map(|w| format!("{w:#x}")).collect();
                format!("WRITE {addr} x{count} ({}) [{}{}]", register_name(*addr), shown.join(", "), if data.len() > 8 { ", ..." } else { "" })
            }
        },
    }
}

/// Coarse class of a request vector, for counting what a capture contains.
pub fn request_kind(vector: &[u32]) -> String {
    match vector {
        [] => "(empty)".into(),
        [0x30, addr, count, ..] => format!("READ {addr} x{count}"),
        [0x40, REG_FIFO_DATA, _, ..] => "queue frame".into(),
        [0x40, REG_FIFO_CONTROL, _, v, ..] => format!("queue control {}", fifo_control_name(*v)),
        [0x40, REG_COMMAND, _, 9999, sub, ..] => format!("CMD misc/{}", misc_name(*sub)),
        [0x40, REG_COMMAND, _, sub, ..] => format!("CMD {}", command_name(*sub)),
        [0x40, addr, ..] => format!("WRITE {addr}"),
        [0x26, addr, ..] => format!("BYTEBLOCK {addr}"),
        [0x20, ..] => "STREAM20".into(),
        [0x21, ..] => "STREAM21".into(),
        [func, ..] => format!("func {func:#04x}"),
    }
}

/// Whether this crate could have built the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A read inside the read allow-list.
    ReadAllowed,
    /// A write the crate can build; the gate asks for this arming level.
    WriteAllowed(Policy),
    /// Laser-class data: decodable, never buildable.
    LaserData,
    /// Refused by the allow-lists, with the reason.
    Refused(Forbidden),
    /// The frame is not a request this module understands (a reply, a damaged or short frame).
    NotARequest,
}

/// The result of [`dissect_request`] and [`dissect_reply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dissection {
    /// Sequence number, if the frame is long enough to have one.
    pub seq: Option<u16>,
    /// Function byte.
    pub func: Option<u8>,
    /// The stored checksum matches in one of the two byte orders.
    pub crc_ok: bool,
    /// The length field matches the frame.
    pub length_ok: bool,
    /// Vendor-log vector of a request.
    pub vector: Option<Vec<u32>>,
    /// One line of text.
    pub text: String,
    /// Whether the crate could have built it.
    pub verdict: Verdict,
}

/// Classify a request vector against the crate's allow-lists (default limits, no known output word).
pub fn verdict_of(vector: &[u32]) -> Verdict {
    match vector {
        [0x30, addr, count] => match check_read(*addr, *count) {
            Ok(()) => Verdict::ReadAllowed,
            Err(e) => Verdict::Refused(e),
        },
        [0x40, addr, _, data @ ..] => match SafeWrite::from_raw_for_simulator(*addr, data, &SafetyConfig::default(), None) {
            Ok(w) if w.policy() == Policy::Laser => Verdict::LaserData,
            Ok(w) => Verdict::WriteAllowed(w.policy()),
            Err(e) => Verdict::Refused(e),
        },
        [func @ (0x26 | 0x20 | 0x21), ..] => Verdict::Refused(Forbidden::Function(*func as u8)),
        _ => Verdict::NotARequest,
    }
}

/// Dissect a datagram sent by the PC.
pub fn dissect_request(datagram: &[u8]) -> Dissection {
    let raw = RawFrame::parse(datagram);
    let vector = raw.request_vector();
    let (text, verdict) = match &vector {
        Some(v) => (describe_vector(v), verdict_of(v)),
        None => (format!("not a request ({} bytes)", datagram.len()), Verdict::NotARequest),
    };
    Dissection { seq: raw.seq, func: raw.func, crc_ok: raw.crc_ok(), length_ok: raw.length_ok(), vector, text, verdict }
}

/// What an exception code means, as far as the logs show.
pub fn exception_text(code: u8) -> &'static str {
    match code {
        2 => "not ready, or the address is not served (unconfirmed)",
        3 => "busy, or the command was refused (unconfirmed)",
        _ => "no known meaning",
    }
}

/// Dissect a datagram sent by the card.
pub fn dissect_reply(datagram: &[u8]) -> Dissection {
    let raw = RawFrame::parse(datagram);
    let p = &raw.payload;
    let text = match (raw.func, raw.exception_code()) {
        (Some(f), Some(code)) => {
            format!("EXCEPTION to {} code {code} (ErrCode {}): {}", func_name(f), exception_errcode(code), exception_text(code))
        }
        (Some(FUNC_READ), _) if p.len() >= 6 => {
            let (addr, count) = (u32::from_le_bytes([p[0], p[1], p[2], p[3]]), u16::from_le_bytes([p[4], p[5]]));
            format!("READ reply {addr} x{count} ({})", register_name(addr))
        }
        (Some(FUNC_WRITE), _) if p.len() >= 6 => {
            let (addr, count) = (u32::from_le_bytes([p[0], p[1], p[2], p[3]]), u16::from_le_bytes([p[4], p[5]]));
            format!("WRITE acknowledged {addr} x{count} ({})", register_name(addr))
        }
        (Some(f), _) => format!("{} reply, {} payload bytes", func_name(f), p.len()),
        (None, _) => format!("not a reply ({} bytes)", datagram.len()),
    };
    Dissection { seq: raw.seq, func: raw.func, crc_ok: raw.crc_ok(), length_ok: raw.length_ok(), vector: None, text, verdict: Verdict::NotARequest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{self, MachineParams};
    use crate::crc::CrcOrder;
    use crate::fifo::FifoItem;
    use crate::frame::{Request, build_frame};

    fn write_frame(addr: u32, words: &[u32]) -> Vec<u8> {
        let mut p = addr.to_le_bytes().to_vec();
        p.extend_from_slice(&(words.len() as u16).to_le_bytes());
        for w in words {
            p.extend_from_slice(&w.to_le_bytes());
        }
        build_frame(1, 0x40, &p, CrcOrder::HiFirst).unwrap()
    }

    #[test]
    fn the_logged_vectors_read_as_what_they_are() {
        let p = MachineParams::default();
        let v = command::jog_continuous(0, true, &p, None).unwrap().request_vector();
        assert_eq!(
            describe_vector(&v),
            "CMD jog [3, 0x0, 200000, 5999, 59990, 4000000]  axis X distance +4000.000 mm (relative), v 200.000 mm/s, a 5999 mm/s^2, jerk 59990"
        );
        let v = command::go_to(430.178, 174.097, &p).unwrap().request_vector();
        assert!(describe_vector(&v).contains("absolute move of X|Y to X +430.178 mm, Y +174.097 mm"), "{}", describe_vector(&v));
        let v = command::stop_all(0, &p).unwrap().request_vector();
        assert_eq!(describe_vector(&v), "CMD stop [1, 0x1f, 2, 2000, 20000]  stop axes X|Y|Y2|Z|W, decel 2000 (jerk 20000), word2=2");
        assert_eq!(describe_vector(&[0x40, 0x65, 2, 1, 0x1f]), "CMD stop [1, 0x1f]  stop axes X|Y|Y2|Z|W, card default decel");
        assert_eq!(describe_vector(&[0x40, 0x65, 3, 2, 1, 0]), "CMD home [2, 0x1, 0]  home axes X");
        assert_eq!(describe_vector(&[0x40, 0x65, 3, 2, 0x10003, 0]), "CMD home [2, 0x10003, 0]  system home axes X|Y|bit16");
        assert_eq!(describe_vector(&[0x40, 0x65, 4, 9999, 5, 0, 0]), "CMD misc/connect prologue [9999, 5, 0, 0]  connect prologue (meaning unknown)");
        assert_eq!(describe_vector(&[0x40, 0x65, 4, 9999, 2, 0x101, 0x100]), "CMD misc/outputs 1..10 [9999, 2, 257, 256]  set DO1=0 DO9=1");
        assert_eq!(describe_vector(&[0x40, 0x65, 4, 9999, 13, 0b11, 0b01]), "CMD misc/outputs 11..26 [9999, 13, 3, 1]  set DO11=1 DO12=0");
        assert_eq!(describe_vector(&[0x40, 0x65, 4, 9999, 4, 1, 5000]), "CMD misc/analogue output [9999, 4, 1, 5000]  analogue output 2 = 5000 mV");
        assert_eq!(describe_vector(&[0x40, 0x65, 5, 9999, 0x11, 1234, 0, 0]), "CMD misc/PWM (5 V) [9999, 17, 1234, 0, 0]  PWM 1234 Hz, duty 0");
        assert_eq!(describe_vector(&[0x40, 0x65, 1, 101]), "CMD follower stop [101]");
        assert_eq!(describe_vector(&[0x40, 0x65, 3, 103, 500, 3000]), "CMD follower move [103, 500, 3000]  follower v 50.0 mm/s, height +3.000 mm");
        assert_eq!(describe_vector(&[0x40, 0x67, 1, 2]), "queue control 2 (start)");
        assert_eq!(describe_vector(&[0x30, 1000, 36]), "READ 1000 x36 (status)");
        assert_eq!(describe_vector(&[0x30, 2010, 3]), "READ 2010 x3 (axis_ro + 10)");
        assert_eq!(describe_vector(&[0x40, 150, 1, 5555]), "WRITE 150 x1 (follower factory reset / commit) [0x15b3]");
        assert_eq!(describe_vector(&[0x40, 59502, 1, 1]), "WRITE 59502 x1 (licence / clock area) [0x1]");
        assert_eq!(describe_vector(&[0x26, 0x1000, 512]), "BYTEBLOCK 4096 x512 (firmware or file download)");
        assert_eq!(describe_vector(&[0x21, 7]), "STREAM21 7 bytes");
        assert_eq!(describe_vector(&[0xC0, 3]), "EXCEPTION 0x40 code 3");
        assert_eq!(describe_vector(&[]), "(empty)");
    }

    #[test]
    fn queue_frames_are_summarised() {
        let w = command::fifo_frame(7, &[FifoItem::boundary(), FifoItem::tick(1, 2), FifoItem::tick(3, 4), FifoItem::mode(5)]).unwrap();
        assert_eq!(describe_vector(&w.request_vector()), "queue frame id=7 (0x7), 4 items [tick x2, boundary x1, mode x1]");
        // a tick with duty and a follower record are named, not hidden
        let words = [9u32, 0x0008_0BB8, 0, (5000 << 16) | 40, 0x0008_0067, 500, 3000, 0x0005_0BB8];
        assert_eq!(describe_fifo(&words), "queue frame id=9 (0x9), 2 items [follower move x1, tick x1], 1 ticks with duty, 1 words left over");
        assert_eq!(describe_fifo(&[]), "queue frame (empty: no frame id)");
    }

    #[test]
    fn every_request_gets_a_verdict() {
        let p = MachineParams::default();
        let d = dissect_request(&Request::read(1000, 36).unwrap().encode(4).unwrap());
        assert_eq!((d.seq, d.func, d.crc_ok, d.length_ok), (Some(4), Some(0x30), true, true));
        assert_eq!(d.verdict, Verdict::ReadAllowed);
        let d = dissect_request(&Request::write(command::jog_step(0, 1.0, &p).unwrap()).encode(5).unwrap());
        assert_eq!(d.verdict, Verdict::WriteAllowed(Policy::Motion));
        assert_eq!(dissect_request(&write_frame(0x65, &[1, 0x1f])).verdict, Verdict::WriteAllowed(Policy::Always));
        // laser data and the forbidden writes are named and marked
        assert_eq!(dissect_request(&write_frame(0x65, &[9999, 2, 0x100, 0x100])).verdict, Verdict::LaserData);
        assert_eq!(dissect_request(&write_frame(0x65, &[9999, 3, 1234, 30])).verdict, Verdict::LaserData);
        for (addr, words) in
            [(150u32, &[5555u32][..]), (151, &[0]), (59502, &[1]), (0x65, &[0x65]), (0x65, &[0x66]), (0x65, &[107]), (100, &[8888]), (11000, &[1])]
        {
            let d = dissect_request(&write_frame(addr, words));
            assert!(matches!(d.verdict, Verdict::Refused(_)), "{addr} {words:?}: {:?}", d.verdict);
            assert!(d.vector.is_some() && !d.text.is_empty());
        }
        assert_eq!(dissect_request(&write_frame(150, &[9999])).verdict, Verdict::Refused(Forbidden::Register150Or151(150)));
        // licence reads
        let rd = build_frame(3, 0x30, &[0x6c, 0xe8, 0, 0, 1, 0], CrcOrder::HiFirst).unwrap();
        assert_eq!(dissect_request(&rd).verdict, Verdict::Refused(Forbidden::LicenceArea(59500)));
        // a byte block is named and refused
        let mut bb = 0x1000u32.to_le_bytes().to_vec();
        bb.extend_from_slice(&2u16.to_le_bytes());
        bb.extend_from_slice(&[1, 2]);
        let d = dissect_request(&build_frame(1, 0x26, &bb, CrcOrder::HiFirst).unwrap());
        assert_eq!(d.verdict, Verdict::Refused(Forbidden::Function(0x26)));
        // garbage
        assert_eq!(dissect_request(&[1, 2, 3]).verdict, Verdict::NotARequest);
        assert_eq!(dissect_request(&[]).verdict, Verdict::NotARequest);
    }

    #[test]
    fn replies_are_described() {
        // the logged exception frames
        let d = dissect_reply(&[0x1d, 0x3f, 0x34, 0x45, 0x00, 0x03, 0x00, 0xc0, 0x03]);
        assert_eq!(d.text, "EXCEPTION to WRITE code 3 (ErrCode 503): busy, or the command was refused (unconfirmed)");
        assert!(d.crc_ok && d.length_ok);
        let d = dissect_reply(&[0x06, 0xc5, 0xd0, 0x45, 0x00, 0x03, 0x00, 0xb0, 0x02]);
        assert_eq!(d.text, "EXCEPTION to READ code 2 (ErrCode 502): not ready, or the address is not served (unconfirmed)");
        let rep = crate::frame::Reply::Read { addr: 50000, words: vec![0; 26] }.encode(9).unwrap();
        assert_eq!(dissect_reply(&rep).text, "READ reply 50000 x26 (system_rw)");
        let rep = crate::frame::Reply::Write { func: 0x40, addr: 0x67, count: 1 }.encode(9).unwrap();
        assert_eq!(dissect_reply(&rep).text, "WRITE acknowledged 103 x1 (motion queue control)");
        assert_eq!(dissect_reply(&[]).text, "not a reply (0 bytes)");
    }

    #[test]
    fn names_cover_the_whole_vocabulary() {
        for sub in [1u32, 2, 3, 4, 5, 7, 101, 102, 103, 104, 107, 109, 117, 118, 9999] {
            assert_ne!(command_name(sub), "unknown", "{sub}");
        }
        for sub in [1u32, 2, 3, 4, 5, 13, 16, 17] {
            assert_ne!(misc_name(sub), "unknown", "{sub}");
        }
        for op in [3000u16, 3001, 3002, 9999, 103, 104, 105, 106, 108, 109, 118, 2001, 2002, 2004, 3] {
            assert_ne!(fifo_opcode_name(op), "unknown", "{op}");
        }
        assert_eq!(command_name(6), "unknown");
        assert_eq!(request_kind(&[0x40, 0x65, 4, 9999, 2, 1, 1]), "CMD misc/outputs 1..10");
        assert_eq!(request_kind(&[0x40, 0x65, 1, 101]), "CMD follower stop");
        assert_eq!(request_kind(&[0x40, 0x66, 3, 1, 2, 3]), "queue frame");
        assert_eq!(request_kind(&[0x40, 0x67, 1, 3]), "queue control stop");
        assert_eq!(request_kind(&[0x30, 1000, 36]), "READ 1000 x36");
        assert_eq!(request_kind(&[0x26, 4096, 3]), "BYTEBLOCK 4096");
        assert_eq!(request_kind(&[]), "(empty)");
        assert_eq!(register_name(59650), "hardware parameters");
        assert_eq!(register_name(12002), "follower calibration table");
        assert_eq!(register_name(7), "unknown");
    }
}
