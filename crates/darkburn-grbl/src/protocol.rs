//! The GRBL wire protocol: real-time bytes, response lines, status reports, banners and
//! override byte sequences.
//!
//! Implemented from the GRBL v1.1 interface documentation (gnea/grbl wiki: "Grbl v1.1
//! Interface", "Commands", "Jogging") and the grblHAL / FluidNC compatibility notes. No
//! firmware or sender source code was used.

use std::fmt;

/// Single real-time command bytes. They are acted on as soon as the controller receives them,
/// never enter the RX buffer, and must not be followed by a newline.
pub mod rt {
    pub const STATUS: u8 = b'?';
    pub const CYCLE_START: u8 = b'~';
    pub const FEED_HOLD: u8 = b'!';
    pub const SOFT_RESET: u8 = 0x18;
    pub const SAFETY_DOOR: u8 = 0x84;
    pub const JOG_CANCEL: u8 = 0x85;
    pub const FEED_100: u8 = 0x90;
    pub const FEED_PLUS_10: u8 = 0x91;
    pub const FEED_MINUS_10: u8 = 0x92;
    pub const FEED_PLUS_1: u8 = 0x93;
    pub const FEED_MINUS_1: u8 = 0x94;
    pub const RAPID_100: u8 = 0x95;
    pub const RAPID_50: u8 = 0x96;
    pub const RAPID_25: u8 = 0x97;
    pub const SPINDLE_100: u8 = 0x99;
    pub const SPINDLE_PLUS_10: u8 = 0x9A;
    pub const SPINDLE_MINUS_10: u8 = 0x9B;
    pub const SPINDLE_PLUS_1: u8 = 0x9C;
    pub const SPINDLE_MINUS_1: u8 = 0x9D;
    pub const SPINDLE_STOP: u8 = 0x9E;
    pub const FLOOD_TOGGLE: u8 = 0xA0;
    pub const MIST_TOGGLE: u8 = 0xA1;
    /// grblHAL: full status report (also used to detect grblHAL).
    pub const HAL_FULL_REPORT: u8 = 0x87;

    /// True for bytes the controller handles in real time.
    pub fn is_realtime(b: u8) -> bool {
        matches!(b, STATUS | CYCLE_START | FEED_HOLD | SOFT_RESET) || b >= 0x80
    }
}

/// Which firmware family is on the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FirmwareKind {
    Grbl,
    GrblHal,
    FluidNc,
}

/// Firmware identity found during the handshake.
#[derive(Clone, Debug, PartialEq)]
pub struct FirmwareInfo {
    pub kind: FirmwareKind,
    /// GRBL-compatible version as reported, e.g. `1.1h` or `0.9j`.
    pub version: String,
    pub major: u16,
    pub minor: u16,
    pub letter: Option<char>,
    /// The banner line exactly as received.
    pub banner: String,
    /// Extra build string, e.g. `FluidNC v3.9.7` or `[VER:1.1h.20190825:]` contents.
    pub build: String,
    /// Usable RX buffer size from `[OPT:...]`, or the documented default (128).
    pub rx_buffer_size: usize,
    /// Planner blocks from `[OPT:...]`, if reported.
    pub planner_blocks: Option<u16>,
    /// Option letters from `[OPT:...]`.
    pub options: String,
}

impl FirmwareInfo {
    /// Version before 1.1f: LightBurn's "GRBL-M3" class. M4 dynamic power is not reliable, so
    /// jobs should use M3 constant power.
    pub fn m3_only(&self) -> bool {
        self.kind == FirmwareKind::Grbl && (self.major, self.minor, self.letter.unwrap_or('a')) < (1, 1, 'f')
    }
    /// `$J=` jogging, jog cancel and real-time overrides (GRBL 1.1 and later).
    pub fn is_v11(&self) -> bool {
        self.kind != FirmwareKind::Grbl || (self.major, self.minor) >= (1, 1)
    }
    /// Short human name, e.g. "GRBL 1.1h", "grblHAL 1.1f", "FluidNC v3.9.7".
    pub fn display_name(&self) -> String {
        match self.kind {
            FirmwareKind::Grbl => format!("GRBL {}", self.version),
            FirmwareKind::GrblHal => format!("grblHAL {}", self.version),
            FirmwareKind::FluidNc => {
                if self.build.is_empty() {
                    format!("FluidNC (GRBL {})", self.version)
                } else {
                    self.build.clone()
                }
            }
        }
    }
}

/// Parses `1.1h`, `0.9j`, `3.9` into (major, minor, letter).
pub fn parse_version(v: &str) -> Option<(u16, u16, Option<char>)> {
    let v = v.trim();
    let (major, rest) = v.split_once('.')?;
    let major: u16 = major.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let minor: u16 = digits.parse().ok()?;
    let letter = rest[digits.len()..].chars().next().filter(|c| c.is_ascii_alphabetic());
    Some((major, minor, letter))
}

/// Recognises a start-up banner:
/// `Grbl 1.1h ['$' for help]`, `GrblHAL 1.1f ['$' or '$HELP' for help]`,
/// `Grbl 3.9 [FluidNC v3.9.7 (noradio) '$' for help]`, `Grbl 0.9j ['$' for help]`.
pub fn parse_banner(line: &str) -> Option<FirmwareInfo> {
    let line = line.trim();
    let (kind, rest) =
        if let Some(r) = line.strip_prefix("GrblHAL ") { (FirmwareKind::GrblHal, r) } else { (FirmwareKind::Grbl, line.strip_prefix("Grbl ")?) };
    let ver = rest.split_whitespace().next()?;
    let (major, minor, letter) = parse_version(ver)?;
    let mut info = FirmwareInfo {
        kind,
        version: ver.to_owned(),
        major,
        minor,
        letter,
        banner: line.to_owned(),
        build: String::new(),
        rx_buffer_size: 128,
        planner_blocks: None,
        options: String::new(),
    };
    if let Some(i) = rest.find("FluidNC") {
        info.kind = FirmwareKind::FluidNc;
        info.build = rest[i..].split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    }
    if info.kind == FirmwareKind::GrblHal {
        info.rx_buffer_size = 1024;
    }
    Some(info)
}

/// The machine state from a status report.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MachineState {
    #[default]
    Unknown,
    Idle,
    /// `Run` (grblHAL may add `:1` hold pending, `:2` probing).
    Run(u8),
    /// `Hold:0` hold complete, `Hold:1` decelerating.
    Hold(u8),
    Jog,
    /// grblHAL may report `Alarm:<code>`.
    Alarm(Option<u16>),
    /// `Door:0` closed, `:1` ajar, `:2` opening/holding, `:3` closing/resuming.
    Door(u8),
    Check,
    Home,
    Sleep,
    Tool,
    Other(String),
}

impl MachineState {
    pub fn parse(s: &str) -> Self {
        let (name, sub) = match s.split_once(':') {
            Some((n, sub)) => (n, sub.parse::<u16>().ok()),
            None => (s, None),
        };
        let sub8 = sub.map(|v| v.min(255) as u8).unwrap_or(0);
        match name {
            "Idle" => Self::Idle,
            "Run" => Self::Run(sub8),
            "Hold" | "Queue" => Self::Hold(if name == "Queue" { 0 } else { sub8 }),
            "Jog" => Self::Jog,
            "Alarm" => Self::Alarm(sub),
            "Door" => Self::Door(sub8),
            "Check" => Self::Check,
            "Home" => Self::Home,
            "Sleep" => Self::Sleep,
            "Tool" => Self::Tool,
            other => Self::Other(other.to_owned()),
        }
    }
    pub fn is_alarm(&self) -> bool {
        matches!(self, Self::Alarm(_))
    }
    /// The machine is (or may be) moving.
    pub fn is_moving(&self) -> bool {
        matches!(self, Self::Run(_) | Self::Jog | Self::Home | Self::Hold(1) | Self::Door(2) | Self::Door(3))
    }
    pub fn name(&self) -> String {
        match self {
            Self::Unknown => "Unknown".into(),
            Self::Idle => "Idle".into(),
            Self::Run(0) => "Run".into(),
            Self::Run(n) => format!("Run:{n}"),
            Self::Hold(n) => format!("Hold:{n}"),
            Self::Jog => "Jog".into(),
            Self::Alarm(None) => "Alarm".into(),
            Self::Alarm(Some(c)) => format!("Alarm:{c}"),
            Self::Door(n) => format!("Door:{n}"),
            Self::Check => "Check".into(),
            Self::Home => "Home".into(),
            Self::Sleep => "Sleep".into(),
            Self::Tool => "Tool".into(),
            Self::Other(s) => s.clone(),
        }
    }
}

impl fmt::Display for MachineState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Up to four axes (X, Y, Z, A/U). Missing axes are 0.
pub type Axes = [f64; 4];

/// One parsed status report. Fields the controller did not send are `None`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct StatusReport {
    pub state: MachineState,
    pub mpos: Option<Axes>,
    pub wpos: Option<Axes>,
    pub wco: Option<Axes>,
    /// Number of axes in the position field.
    pub axes: usize,
    pub feed: Option<f64>,
    pub spindle: Option<f64>,
    /// `Bf:` free planner blocks, free RX bytes.
    pub buffer: Option<(u32, u32)>,
    pub line_number: Option<u32>,
    /// `Ov:` feed %, rapid %, spindle %.
    pub overrides: Option<(u16, u16, u16)>,
    /// `Pn:` triggered pins (letters).
    pub pins: Option<String>,
    /// `A:` accessory state letters.
    pub accessories: Option<String>,
}

fn parse_axes(s: &str) -> Option<(Axes, usize)> {
    let mut out = [0.0; 4];
    let mut n = 0;
    for (i, part) in s.split(',').enumerate() {
        // "nan" and "inf" read as numbers; a position is neither.
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
        if i < 4 {
            out[i] = v;
        }
        n += 1;
    }
    Some((out, n))
}

/// Parses a status report in the GRBL 1.1 grammar (`<Idle|MPos:...|FS:...>`) or the older
/// 0.9 grammar (`<Idle,MPos:0.000,0.000,0.000,WPos:...>`).
pub fn parse_status(line: &str) -> Option<StatusReport> {
    let body = line.trim().strip_prefix('<')?.strip_suffix('>')?;
    if body.contains('|') || !body.contains(',') { parse_status_v11(body) } else { parse_status_v09(body) }
}

fn parse_status_v11(body: &str) -> Option<StatusReport> {
    let mut parts = body.split('|');
    let mut r = StatusReport { state: MachineState::parse(parts.next()?), ..Default::default() };
    for field in parts {
        let Some((key, val)) = field.split_once(':') else {
            continue;
        };
        match key {
            "MPos" => {
                let (a, n) = parse_axes(val)?;
                r.mpos = Some(a);
                r.axes = n;
            }
            "WPos" => {
                let (a, n) = parse_axes(val)?;
                r.wpos = Some(a);
                r.axes = n;
            }
            "WCO" => r.wco = parse_axes(val).map(|(a, _)| a),
            "FS" | "F" => {
                let mut it = val.split(',');
                r.feed = it.next().and_then(|v| v.parse().ok());
                r.spindle = it.next().and_then(|v| v.parse().ok());
            }
            "Bf" => {
                let mut it = val.split(',').map(|v| v.parse::<u32>().ok());
                if let (Some(Some(a)), Some(Some(b))) = (it.next(), it.next()) {
                    r.buffer = Some((a, b));
                }
            }
            "Ln" => r.line_number = val.parse().ok(),
            "Ov" => {
                let v: Vec<u16> = val.split(',').filter_map(|x| x.parse().ok()).collect();
                if v.len() >= 3 {
                    r.overrides = Some((v[0], v[1], v[2]));
                }
            }
            "Pn" => r.pins = Some(val.to_owned()),
            "A" => r.accessories = Some(val.to_owned()),
            _ => {}
        }
    }
    Some(r)
}

fn parse_status_v09(body: &str) -> Option<StatusReport> {
    let mut tokens = body.split(',');
    let mut r = StatusReport { state: MachineState::parse(tokens.next()?), ..Default::default() };
    let mut key = String::new();
    let mut vals: Vec<f64> = Vec::new();
    let flush = |key: &str, vals: &mut Vec<f64>, r: &mut StatusReport| {
        let mut a = [0.0; 4];
        for (i, v) in vals.iter().take(4).enumerate() {
            a[i] = *v;
        }
        match key {
            "MPos" => {
                r.mpos = Some(a);
                r.axes = vals.len();
            }
            "WPos" => {
                r.wpos = Some(a);
                r.axes = vals.len();
            }
            "Buf" => r.buffer = vals.first().map(|v| (*v as u32, 0)),
            "Ln" => r.line_number = vals.first().map(|v| *v as u32),
            _ => {}
        }
        vals.clear();
    };
    for t in tokens {
        let v = match t.split_once(':') {
            Some((k, v)) => {
                flush(&key, &mut vals, &mut r);
                key = k.to_owned();
                v
            }
            None => t,
        };
        match v.parse::<f64>() {
            Ok(x) if x.is_finite() => vals.push(x),
            // A position with a value that cannot be read is no position: leaving the value
            // out would move the other axes up one place.
            _ if matches!(key.as_str(), "MPos" | "WPos") => return None,
            _ => {}
        }
    }
    flush(&key, &mut vals, &mut r);
    Some(r)
}

/// One line received from the controller.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Ok,
    Error(u16),
    /// GRBL 0.9 text errors (`error: Bad number format`).
    ErrorText(String),
    Alarm(u16),
    /// GRBL 0.9 text alarms (`ALARM: Hard/soft limit`).
    AlarmText(String),
    Status(StatusReport),
    Banner(FirmwareInfo),
    /// `$N=value` from `$$`.
    Setting {
        number: u16,
        value: String,
    },
    /// `[MSG:...]`.
    Message(String),
    /// Other bracketed feedback: `[VER:...]`, `[OPT:...]`, `[GC:...]`, `[G54:...]`, `[FIRMWARE:...]`, ...
    Feedback {
        tag: String,
        body: String,
    },
    /// `>G54G20:ok` start-up line result.
    StartupLine(String),
    Other(String),
}

/// Classifies one received line (without its line ending).
pub fn parse_response(line: &str) -> Response {
    let line = line.trim();
    if line == "ok" {
        return Response::Ok;
    }
    if let Some(rest) = line.strip_prefix("error:") {
        return match rest.trim().parse() {
            Ok(n) => Response::Error(n),
            Err(_) => Response::ErrorText(rest.trim().to_owned()),
        };
    }
    if let Some(rest) = line.strip_prefix("ALARM:") {
        return match rest.trim().parse() {
            Ok(n) => Response::Alarm(n),
            Err(_) => Response::AlarmText(rest.trim().to_owned()),
        };
    }
    if line.starts_with('<')
        && line.ends_with('>')
        && let Some(s) = parse_status(line)
    {
        return Response::Status(s);
    }
    if let Some(info) = parse_banner(line) {
        return Response::Banner(info);
    }
    if let Some(rest) = line.strip_prefix('$')
        && let Some((n, v)) = rest.split_once('=')
        && let Ok(number) = n.parse()
    {
        // GRBL 0.9 appends a comment: `$0=10 (step pulse, usec)`.
        let value = v.split_whitespace().next().unwrap_or("").to_owned();
        return Response::Setting { number, value };
    }
    if let Some(body) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        let (tag, rest) = body.split_once(':').unwrap_or((body, ""));
        if tag == "MSG" {
            return Response::Message(rest.to_owned());
        }
        return Response::Feedback { tag: tag.to_owned(), body: rest.to_owned() };
    }
    if line.starts_with('>') {
        return Response::StartupLine(line.to_owned());
    }
    Response::Other(line.to_owned())
}

/// Applies the `[VER:...]`, `[OPT:...]` and `[FIRMWARE:...]` lines from `$I` to `info`.
pub fn apply_build_info(info: &mut FirmwareInfo, tag: &str, body: &str) {
    match tag {
        "VER" => {
            // `1.1h.20190825:` or `3.9 FluidNC v3.9.7:` or grblHAL `1.1f.20240101:`
            let body = body.trim_end_matches(':');
            if let Some(i) = body.find("FluidNC") {
                info.kind = FirmwareKind::FluidNc;
                info.build = body[i..].split(':').next().unwrap_or("").trim().to_owned();
            } else if info.build.is_empty() {
                info.build = body.to_owned();
            }
        }
        "OPT" => {
            let mut it = body.split(',');
            info.options = it.next().unwrap_or("").to_owned();
            if let Some(b) = it.next().and_then(|v| v.trim().parse().ok()) {
                info.planner_blocks = Some(b);
            }
            if let Some(rx) = it.next().and_then(|v| v.trim().parse::<usize>().ok())
                && rx >= 16
            {
                info.rx_buffer_size = rx;
            }
        }
        "FIRMWARE" if body.contains("grblHAL") => {
            info.kind = FirmwareKind::GrblHal;
        }
        _ => {}
    }
}

/// Strips `;` and `( )` comments and surrounding white space. Returns `None` for lines that
/// carry nothing to send.
pub fn clean_line(line: &str) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut in_paren = false;
    for c in line.chars() {
        match c {
            '(' => in_paren = true,
            ')' if in_paren => in_paren = false,
            ';' if !in_paren => break,
            '\r' | '\n' => {}
            _ if !in_paren => out.push(c),
            _ => {}
        }
    }
    let t = out.trim();
    if t.is_empty() { None } else { Some(t.to_owned()) }
}

/// True when a line could move the machine: a jog, a motion G-code, or axis words without
/// a non-motion command (G10, G92, G28.1, G30.1, G43.1) that uses them.
pub fn is_motion_line(line: &str) -> bool {
    let up = line.trim().to_ascii_uppercase();
    if up.starts_with("$J=") {
        return true;
    }
    if up.starts_with('$') {
        return false;
    }
    let words = words(&up);
    let has_axis = words.iter().any(|(l, _)| matches!(l, 'X' | 'Y' | 'Z' | 'A' | 'B' | 'C'));
    let mut motion_g = false;
    let mut uses_axes_otherwise = false;
    for (l, v) in &words {
        if *l == 'G' {
            match v.as_str() {
                "0" | "00" | "1" | "01" | "2" | "02" | "3" | "03" | "28" | "30" | "38.2" | "38.3" | "38.4" | "38.5" => motion_g = true,
                "10" | "92" | "28.1" | "30.1" | "43.1" => uses_axes_otherwise = true,
                _ => {}
            }
        }
    }
    motion_g || (has_axis && !uses_axes_otherwise)
}

/// Splits a G-code line into (letter, number text) words. Comments must already be removed.
pub fn words(line: &str) -> Vec<(char, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].to_ascii_uppercase();
        i += 1;
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | '-' | '+')) {
            i += 1;
        }
        out.push((c, chars[start..i].iter().collect()));
    }
    out
}

/// Real-time bytes that set the feed override to `percent` (clamped to 10-200 %):
/// reset to 100 %, then ±10 % and ±1 % steps.
pub fn feed_override_bytes(percent: u16) -> Vec<u8> {
    override_bytes(percent, [rt::FEED_100, rt::FEED_PLUS_10, rt::FEED_MINUS_10, rt::FEED_PLUS_1, rt::FEED_MINUS_1])
}

/// Real-time bytes that set the laser power (spindle) override to `percent` (10-200 %).
pub fn power_override_bytes(percent: u16) -> Vec<u8> {
    override_bytes(percent, [rt::SPINDLE_100, rt::SPINDLE_PLUS_10, rt::SPINDLE_MINUS_10, rt::SPINDLE_PLUS_1, rt::SPINDLE_MINUS_1])
}

fn override_bytes(percent: u16, [reset, p10, m10, p1, m1]: [u8; 5]) -> Vec<u8> {
    let target = i32::from(percent.clamp(10, 200));
    let delta = target - 100;
    let mut out = vec![reset];
    let (ten, one) = if delta >= 0 { (p10, p1) } else { (m10, m1) };
    let d = delta.abs();
    out.extend(std::iter::repeat_n(ten, (d / 10) as usize));
    out.extend(std::iter::repeat_n(one, (d % 10) as usize));
    out
}

/// The three rapid override levels GRBL offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RapidOverride {
    Full,
    Half,
    Quarter,
}

impl RapidOverride {
    pub fn byte(self) -> u8 {
        match self {
            Self::Full => rt::RAPID_100,
            Self::Half => rt::RAPID_50,
            Self::Quarter => rt::RAPID_25,
        }
    }
    pub fn percent(self) -> u16 {
        match self {
            Self::Full => 100,
            Self::Half => 50,
            Self::Quarter => 25,
        }
    }
}

/// Formats a coordinate for G-code: three decimals, trailing zeros removed.
pub fn fmt_num(v: f64) -> String {
    let v = if v.abs() < 0.0005 { 0.0 } else { v };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banners() {
        let g = parse_banner("Grbl 1.1h ['$' for help]").unwrap();
        assert_eq!((g.kind, g.major, g.minor, g.letter), (FirmwareKind::Grbl, 1, 1, Some('h')));
        assert!(!g.m3_only());
        assert!(g.is_v11());
        let e = parse_banner("Grbl 1.1e ['$' for help]").unwrap();
        assert!(e.m3_only());
        let old = parse_banner("Grbl 0.9j ['$' for help]").unwrap();
        assert!(old.m3_only());
        assert!(!old.is_v11());
        let hal = parse_banner("GrblHAL 1.1f ['$' or '$HELP' for help]").unwrap();
        assert_eq!(hal.kind, FirmwareKind::GrblHal);
        assert_eq!(hal.rx_buffer_size, 1024);
        let f = parse_banner("Grbl 3.9 [FluidNC v3.9.7 (noradio) '$' for help]").unwrap();
        assert_eq!(f.kind, FirmwareKind::FluidNc);
        assert_eq!(f.build, "FluidNC v3.9.7");
        assert!(parse_banner("ok").is_none());
    }

    #[test]
    fn status_v11() {
        let r = parse_status("<Hold:1|MPos:10.000,-2.500,0.000|Bf:15,128|FS:1200,500|WCO:1.000,2.000,0.000|Ov:110,50,90|Pn:XD|A:S>").unwrap();
        assert_eq!(r.state, MachineState::Hold(1));
        assert_eq!(r.mpos.unwrap()[..3], [10.0, -2.5, 0.0]);
        assert_eq!(r.axes, 3);
        assert_eq!(r.buffer, Some((15, 128)));
        assert_eq!((r.feed, r.spindle), (Some(1200.0), Some(500.0)));
        assert_eq!(r.wco.unwrap()[..2], [1.0, 2.0]);
        assert_eq!(r.overrides, Some((110, 50, 90)));
        assert_eq!(r.pins.as_deref(), Some("XD"));
        assert_eq!(r.accessories.as_deref(), Some("S"));
        let a = parse_status("<Alarm:11|WPos:1.000,2.000,3.000,4.000>").unwrap();
        assert_eq!(a.state, MachineState::Alarm(Some(11)));
        assert_eq!(a.axes, 4);
    }

    #[test]
    fn status_v09() {
        let r = parse_status("<Run,MPos:1.000,2.000,3.000,WPos:0.500,1.000,1.500,Buf:3>").unwrap();
        assert_eq!(r.state, MachineState::Run(0));
        assert_eq!(r.mpos.unwrap()[..3], [1.0, 2.0, 3.0]);
        assert_eq!(r.wpos.unwrap()[..3], [0.5, 1.0, 1.5]);
        assert_eq!(r.buffer, Some((3, 0)));
    }

    #[test]
    fn responses() {
        assert_eq!(parse_response("ok"), Response::Ok);
        assert_eq!(parse_response("error:22"), Response::Error(22));
        assert_eq!(parse_response("error: Bad number format"), Response::ErrorText("Bad number format".into()));
        assert_eq!(parse_response("ALARM:2"), Response::Alarm(2));
        assert_eq!(parse_response("$110=6000.000"), Response::Setting { number: 110, value: "6000.000".into() });
        assert_eq!(parse_response("[MSG:Caution: Unlocked]"), Response::Message("Caution: Unlocked".into()));
        assert_eq!(parse_response("[OPT:V,15,128]"), Response::Feedback { tag: "OPT".into(), body: "V,15,128".into() });
        assert!(matches!(parse_response("$N0="), Response::Other(_)));
        assert!(matches!(parse_response(">G21:ok"), Response::StartupLine(_)));
    }

    #[test]
    fn build_info() {
        let mut i = parse_banner("Grbl 1.1f ['$' for help]").unwrap();
        apply_build_info(&mut i, "FIRMWARE", "grblHAL");
        apply_build_info(&mut i, "OPT", "VNMSL,35,1024,3,0");
        assert_eq!(i.kind, FirmwareKind::GrblHal);
        assert_eq!(i.rx_buffer_size, 1024);
        assert_eq!(i.planner_blocks, Some(35));
    }

    #[test]
    fn cleaning_and_motion() {
        assert_eq!(clean_line("G1 X1 ; cut").as_deref(), Some("G1 X1"));
        assert_eq!(clean_line("(hello) G0 X2 (y)").as_deref(), Some("G0 X2"));
        assert_eq!(clean_line("   ; only comment"), None);
        assert!(is_motion_line("G0 X10"));
        assert!(is_motion_line("X10 Y2"));
        assert!(is_motion_line("$J=G91 X1 F100"));
        assert!(!is_motion_line("G10 L20 P1 X0 Y0"));
        assert!(!is_motion_line("G92 X0"));
        assert!(!is_motion_line("M5"));
        assert!(!is_motion_line("G4 P0"));
        assert!(!is_motion_line("$H"));
    }

    #[test]
    fn overrides() {
        assert_eq!(feed_override_bytes(100), vec![rt::FEED_100]);
        assert_eq!(feed_override_bytes(123), [vec![rt::FEED_100, rt::FEED_PLUS_10, rt::FEED_PLUS_10], vec![rt::FEED_PLUS_1; 3]].concat());
        assert_eq!(power_override_bytes(81), [vec![rt::SPINDLE_100], vec![rt::SPINDLE_MINUS_10], vec![rt::SPINDLE_MINUS_1; 9]].concat());
        assert_eq!(feed_override_bytes(5).len(), 1 + 9); // clamped to 10 %
        assert_eq!(fmt_num(1.50), "1.5");
        assert_eq!(fmt_num(-0.0001), "0");
        assert_eq!(fmt_num(400.0), "400");
    }
}
