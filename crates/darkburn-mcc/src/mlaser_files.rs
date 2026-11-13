//! The small files of an Mlaser machine package: PWM (scan) compensation tables, `.ini` files and
//! the vendor's `scFlie` value lists (`.dat`, `.pcf` and some `.ini` files).
//!
//! What each file is and what is known about its values is in `docs/research/mlaser-files.md`.
//! Every reader here keeps the text it does not understand, never panics, and refuses input larger
//! than [`MAX_SMALL_FILE_BYTES`].
//!
//! * [`CompensationTable`]: `pwmCompensation.txt` (fibre) and `Co2_pwmCompensation.txt` (CO2), one
//!   `speed,offset` pair per line. The same pairs are stored in the machine backup as
//!   `FiberScanFlyCompensateStr` / `CO2ScanFlyCompensateStr` (`100#0.34,200#0.56,...`). The first
//!   column is read as speed in mm/s and the second as an offset in mm, for the speed-dependent
//!   lag of the laser during fly (scan) cutting. This meaning is **inferred, not confirmed**: the
//!   second column could also be a time.
//! * [`IniFile`]: an order- and text-keeping `.ini` reader (`softPara.ini`, `ipAdd.ini`), with
//!   [`SoftPara`] and [`NetSettings`] views.
//! * [`ScFile`]: a value list framed by the lines `scFlie` and `eof`. `ManuContour.dat` is a
//!   count followed by that many values ([`ScFile::counted`]); `PithCompensate.pcf`,
//!   `AutosaveParam*.ini` and `Temp/tempIsBreak.ini` are plain lists whose meaning is open.
//! * [`read_statistics`]: `ProcessesStatistic.txt`, comma-separated job history in code page 936.

use crate::paramxml::{LineEnding, MlaserError, TextEncoding, decode_text, encode_text, parse_number};

/// Largest small file accepted, bytes.
pub const MAX_SMALL_FILE_BYTES: usize = 1024 * 1024;
/// First line of a value list (sic, as the vendor writes it).
pub const SC_MAGIC: &str = "scFlie";
/// Last line of a value list.
pub const SC_EOF: &str = "eof";

fn split_lines(text: &str) -> (Vec<&str>, LineEnding, bool) {
    let le = LineEnding::detect(text);
    let trailing = text.ends_with('\n');
    let body = text.strip_suffix('\n').map(|t| t.strip_suffix('\r').unwrap_or(t)).unwrap_or(text);
    let lines = if body.is_empty() && trailing { vec![""] } else { body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect() };
    let lines = if text.is_empty() { Vec::new() } else { lines };
    (lines, le, trailing)
}

fn join_lines(lines: &[String], le: LineEnding, trailing: bool) -> String {
    let mut s = lines.join(le.as_str());
    if trailing && !lines.is_empty() {
        s.push_str(le.as_str());
    }
    s
}

fn check_size(bytes: &[u8]) -> Result<(), MlaserError> {
    if bytes.len() > MAX_SMALL_FILE_BYTES {
        return Err(MlaserError::new(format!("file is {} bytes, more than the {MAX_SMALL_FILE_BYTES} accepted", bytes.len())));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Compensation tables

/// One row of a compensation table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompPoint {
    /// First column: speed, mm/s (inferred).
    pub speed_mm_s: f64,
    /// Second column: offset, mm (inferred; could be a time).
    pub offset_mm: f64,
}

/// A PWM (scan) compensation table.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CompensationTable {
    /// Rows in file order.
    pub points: Vec<CompPoint>,
    /// Line ending used when writing.
    pub line_ending: LineEnding,
    /// Whether the file ends with a line ending (the vendor's files do not).
    pub trailing_newline: bool,
}

impl CompensationTable {
    /// Parse a `speed,offset` per line file. Blank lines are skipped; any other line that is not
    /// two finite numbers is an error.
    pub fn parse(bytes: &[u8]) -> Result<CompensationTable, MlaserError> {
        check_size(bytes)?;
        let (text, _) = decode_text(bytes);
        let (lines, line_ending, trailing_newline) = split_lines(&text);
        let mut points = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if l.trim().is_empty() {
                continue;
            }
            let (a, b) = l.split_once(',').ok_or_else(|| MlaserError::at(i + 1, format!("{l:?} is not speed,offset")))?;
            match (parse_number(a), parse_number(b)) {
                (Some(speed_mm_s), Some(offset_mm)) => points.push(CompPoint { speed_mm_s, offset_mm }),
                _ => return Err(MlaserError::at(i + 1, format!("{l:?} is not two numbers"))),
            }
        }
        if points.is_empty() {
            return Err(MlaserError::new("no rows"));
        }
        Ok(CompensationTable { points, line_ending, trailing_newline })
    }

    /// Parse the backup's form, `100#0.34,200#0.56,...`.
    pub fn from_scan_fly_str(s: &str) -> Result<CompensationTable, MlaserError> {
        let mut points = Vec::new();
        for (i, part) in s.split(',').enumerate().filter(|(_, p)| !p.trim().is_empty()) {
            let (a, b) = part.split_once('#').ok_or_else(|| MlaserError::new(format!("item {} {part:?} is not speed#offset", i + 1)))?;
            match (parse_number(a), parse_number(b)) {
                (Some(speed_mm_s), Some(offset_mm)) => points.push(CompPoint { speed_mm_s, offset_mm }),
                _ => return Err(MlaserError::new(format!("item {} {part:?} is not two numbers", i + 1))),
            }
            if points.len() > 100_000 {
                return Err(MlaserError::new("too many items"));
            }
        }
        Ok(CompensationTable { points, line_ending: LineEnding::CrLf, trailing_newline: false })
    }

    /// The backup's form.
    pub fn to_scan_fly_str(&self) -> String {
        self.points.iter().map(|p| format!("{}#{}", p.speed_mm_s, p.offset_mm)).collect::<Vec<_>>().join(",")
    }

    /// The file text. Numbers are written in their shortest form (`0.364`, `100`).
    pub fn to_text(&self) -> String {
        let lines: Vec<String> = self.points.iter().map(|p| format!("{},{}", p.speed_mm_s, p.offset_mm)).collect();
        join_lines(&lines, self.line_ending, self.trailing_newline)
    }

    /// The offset at a speed: linear between rows (sorted by speed), held flat outside them.
    /// `None` for an empty table or a non-finite speed.
    pub fn offset_at(&self, speed_mm_s: f64) -> Option<f64> {
        if !speed_mm_s.is_finite() {
            return None;
        }
        let mut pts = self.points.clone();
        pts.sort_by(|a, b| a.speed_mm_s.total_cmp(&b.speed_mm_s));
        let first = pts.first()?;
        let last = pts.last()?;
        if speed_mm_s <= first.speed_mm_s {
            return Some(first.offset_mm);
        }
        if speed_mm_s >= last.speed_mm_s {
            return Some(last.offset_mm);
        }
        pts.windows(2).find(|w| speed_mm_s >= w[0].speed_mm_s && speed_mm_s <= w[1].speed_mm_s).map(|w| {
            let span = w[1].speed_mm_s - w[0].speed_mm_s;
            if span <= 0.0 { w[0].offset_mm } else { w[0].offset_mm + (w[1].offset_mm - w[0].offset_mm) * (speed_mm_s - w[0].speed_mm_s) / span }
        })
    }

    /// Rows that differ from `other` (by speed): `(speed, self offset, other offset)`, with `None`
    /// where a speed is missing on one side.
    pub fn differences(&self, other: &CompensationTable) -> Vec<(f64, Option<f64>, Option<f64>)> {
        let mut speeds: Vec<f64> = self.points.iter().chain(&other.points).map(|p| p.speed_mm_s).collect();
        speeds.sort_by(f64::total_cmp);
        speeds.dedup();
        let find = |t: &CompensationTable, s: f64| t.points.iter().find(|p| p.speed_mm_s == s).map(|p| p.offset_mm);
        speeds.into_iter().map(|s| (s, find(self, s), find(other, s))).filter(|(_, a, b)| a != b).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// INI files

/// One line of an `.ini` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IniLine {
    /// A blank line (its text kept).
    Blank(String),
    /// `;` or `#` comment.
    Comment(String),
    /// `[name]`.
    Section {
        /// Section name.
        name: String,
        /// Line as written.
        raw: String,
    },
    /// `key=value`.
    Pair {
        /// Key, trimmed.
        key: String,
        /// Value, trimmed.
        value: String,
        /// Line as written; `None` after a change.
        raw: Option<String>,
    },
    /// Anything else, kept as written.
    Other(String),
}

/// An `.ini` file that keeps every line.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct IniFile {
    /// Lines in file order.
    pub lines: Vec<IniLine>,
    /// Line ending.
    pub line_ending: LineEnding,
    /// Whether the file ends with a line ending.
    pub trailing_newline: bool,
    /// Text encoding.
    pub encoding: TextEncoding,
}

impl IniFile {
    /// Parse.
    pub fn parse(bytes: &[u8]) -> Result<IniFile, MlaserError> {
        check_size(bytes)?;
        let (text, encoding) = decode_text(bytes);
        let (lines, line_ending, trailing_newline) = split_lines(&text);
        let lines = lines
            .into_iter()
            .map(|l| {
                let t = l.trim();
                if t.is_empty() {
                    IniLine::Blank(l.to_string())
                } else if t.starts_with(';') || t.starts_with('#') {
                    IniLine::Comment(l.to_string())
                } else if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                    IniLine::Section { name: name.trim().to_string(), raw: l.to_string() }
                } else if let Some((k, v)) = l.split_once('=') {
                    IniLine::Pair { key: k.trim().to_string(), value: v.trim().to_string(), raw: Some(l.to_string()) }
                } else {
                    IniLine::Other(l.to_string())
                }
            })
            .collect();
        Ok(IniFile { lines, line_ending, trailing_newline, encoding })
    }

    /// Section names in order.
    pub fn sections(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                IniLine::Section { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Pairs of a section (case-insensitive name) in order. Keys before any section belong to
    /// the section `""`.
    pub fn pairs(&self, section: &str) -> Vec<(&str, &str)> {
        let mut cur = "";
        let mut out = Vec::new();
        for l in &self.lines {
            match l {
                IniLine::Section { name, .. } => cur = name.as_str(),
                IniLine::Pair { key, value, .. } if cur.eq_ignore_ascii_case(section) => out.push((key.as_str(), value.as_str())),
                _ => {}
            }
        }
        out
    }

    /// The value of a key (both case-insensitive).
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.pairs(section).into_iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    /// Set a value. A missing key is added at the end of its section, a missing section at the end
    /// of the file.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let mut cur = String::new();
        let mut last_in_section: Option<usize> = None;
        for (i, l) in self.lines.iter_mut().enumerate() {
            match l {
                IniLine::Section { name, .. } => {
                    cur = name.clone();
                    if cur.eq_ignore_ascii_case(section) {
                        last_in_section = Some(i);
                    }
                }
                IniLine::Pair { key: k, value: v, raw } if cur.eq_ignore_ascii_case(section) => {
                    if k.eq_ignore_ascii_case(key) {
                        *v = value.to_string();
                        *raw = None;
                        return;
                    }
                    last_in_section = Some(i);
                }
                _ => {}
            }
        }
        let pair = IniLine::Pair { key: key.to_string(), value: value.to_string(), raw: None };
        match last_in_section {
            Some(i) => self.lines.insert(i + 1, pair),
            // A top-level key with no top-level pairs yet goes before the first section, so it
            // stays at the top level.
            None if section.is_empty() => {
                let at = self.lines.iter().position(|l| matches!(l, IniLine::Section { .. })).unwrap_or(self.lines.len());
                self.lines.insert(at, pair);
            }
            None => {
                if !section.is_empty() {
                    self.lines.push(IniLine::Section { name: section.to_string(), raw: format!("[{section}]") });
                }
                self.lines.push(pair);
            }
        }
    }

    /// The file text; unchanged lines are written as read.
    pub fn to_text(&self) -> String {
        let lines: Vec<String> = self
            .lines
            .iter()
            .map(|l| match l {
                IniLine::Blank(s) | IniLine::Comment(s) | IniLine::Other(s) => s.clone(),
                IniLine::Section { raw, .. } => raw.clone(),
                IniLine::Pair { raw: Some(r), .. } => r.clone(),
                IniLine::Pair { key, value, raw: None } => format!("{key}={value}"),
            })
            .collect();
        join_lines(&lines, self.line_ending, self.trailing_newline)
    }

    /// The file bytes. Refused for a file whose text could not be decoded.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MlaserError> {
        encode_text(&self.to_text(), self.encoding)
    }
}

/// `softPara.ini`, section `[SC2000]`: whether the program last exited normally, and the last
/// axis positions in card units (inferred: `K` = 1000 units per mm, so `XAxis=250000` would be
/// 250 mm).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SoftPara {
    /// `NormalExit`.
    pub normal_exit: Option<bool>,
    /// `XAxis`, `YAxis`, `ZAxis`, `WAxis` in card units.
    pub axes: [Option<i64>; 4],
}

impl SoftPara {
    /// Read from an ini file.
    pub fn from_ini(ini: &IniFile) -> SoftPara {
        let int = |k: &str| ini.get("SC2000", k).and_then(parse_number).filter(|v| v.fract() == 0.0 && v.abs() < 1e15).map(|v| v as i64);
        SoftPara { normal_exit: int("NormalExit").map(|v| v != 0), axes: [int("XAxis"), int("YAxis"), int("ZAxis"), int("WAxis")] }
    }

    /// Axis position in mm for `k` card units per mm.
    pub fn axis_mm(&self, axis: usize, k: i64) -> Option<f64> {
        let v = (*self.axes.get(axis)?)?;
        (k > 0).then(|| v as f64 / k as f64)
    }
}

/// One network endpoint of `ipAdd.ini` (`CardIP`, `CardSubnet`, `CardGateway`, `CardPort`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Prefix (`Card`, `ZF`, `Laser`, ...).
    pub name: String,
    /// Address.
    pub ip: Option<String>,
    /// Subnet mask.
    pub subnet: Option<String>,
    /// Gateway.
    pub gateway: Option<String>,
    /// Port.
    pub port: Option<u16>,
}

/// `ipAdd.ini`: the controller network, read only for display. Nothing in this crate opens a
/// connection.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NetSettings {
    /// Endpoints in file order (every key of `[IP]` ending in `IP`).
    pub endpoints: Vec<Endpoint>,
    /// Timing and other keys of `[IP]` that are not endpoint keys.
    pub other: Vec<(String, String)>,
    /// `[Soft]` keys.
    pub soft: Vec<(String, String)>,
}

impl NetSettings {
    /// Read from an ini file.
    pub fn from_ini(ini: &IniFile) -> NetSettings {
        let ip = ini.pairs("IP");
        let mut names: Vec<String> = Vec::new();
        for (k, _) in &ip {
            if let Some(n) = k.strip_suffix("IP").filter(|n| !n.is_empty())
                && !names.iter().any(|x| x == n)
            {
                names.push(n.to_string());
            }
        }
        let get = |k: String| ip.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v.to_string());
        let endpoints: Vec<Endpoint> = names
            .iter()
            .map(|n| Endpoint {
                name: n.clone(),
                ip: get(format!("{n}IP")),
                subnet: get(format!("{n}Subnet")),
                gateway: get(format!("{n}Gateway")),
                port: get(format!("{n}Port")).and_then(|p| p.trim().parse().ok()),
            })
            .collect();
        let is_endpoint_key = |k: &str| {
            names.iter().any(|n| [format!("{n}IP"), format!("{n}Subnet"), format!("{n}Gateway"), format!("{n}Port")].iter().any(|x| x == k))
        };
        let other = ip.iter().filter(|(k, _)| !is_endpoint_key(k)).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let soft = ini.pairs("Soft").into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        NetSettings { endpoints, other, soft }
    }
}

// ---------------------------------------------------------------------------------------------
// scFlie value lists

/// A value list framed by `scFlie` and `eof`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScFile {
    /// The lines between the frame, as written.
    pub values: Vec<String>,
    /// Line ending.
    pub line_ending: LineEnding,
    /// Whether the file ends with a line ending.
    pub trailing_newline: bool,
    /// Lines after `eof`, kept as written.
    pub after_eof: Vec<String>,
}

/// True if the bytes start like a value list (or a `.chf`, which uses the same first line).
pub fn starts_with_sc_magic(bytes: &[u8]) -> bool {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes).starts_with(SC_MAGIC.as_bytes())
}

impl ScFile {
    /// Parse. The first line must be `scFlie`; a missing `eof` is an error.
    pub fn parse(bytes: &[u8]) -> Result<ScFile, MlaserError> {
        check_size(bytes)?;
        let (text, _) = decode_text(bytes);
        let (lines, line_ending, trailing_newline) = split_lines(&text);
        if lines.first().map(|l| l.trim()) != Some(SC_MAGIC) {
            return Err(MlaserError::at(1, format!("first line is not {SC_MAGIC}")));
        }
        let end = lines.iter().position(|l| l.trim() == SC_EOF).ok_or_else(|| MlaserError::new(format!("no {SC_EOF} line")))?;
        let values = lines.get(1..end).unwrap_or(&[]).iter().map(|s| s.to_string()).collect();
        let after_eof = lines.get(end + 1..).unwrap_or(&[]).iter().map(|s| s.to_string()).collect();
        Ok(ScFile { values, line_ending, trailing_newline, after_eof })
    }

    /// The values as numbers; `None` for a line that is not one.
    pub fn numbers(&self) -> Vec<Option<f64>> {
        self.values.iter().map(|v| parse_number(v.as_str())).collect()
    }

    /// For a count-prefixed list (`ManuContour.dat`): the values after the count, when the count
    /// matches.
    pub fn counted(&self) -> Option<&[String]> {
        let (first, rest) = self.values.split_first()?;
        let n: usize = first.trim().parse().ok()?;
        (n == rest.len()).then_some(rest)
    }

    /// The file text.
    pub fn to_text(&self) -> String {
        let mut lines = vec![SC_MAGIC.to_string()];
        lines.extend(self.values.iter().cloned());
        lines.push(SC_EOF.to_string());
        lines.extend(self.after_eof.iter().cloned());
        join_lines(&lines, self.line_ending, self.trailing_newline)
    }
}

// ---------------------------------------------------------------------------------------------
// Job statistics

/// Read `ProcessesStatistic.txt`: one job per line, comma-separated (name, size, two lengths, a
/// count, a time and a date as written by the vendor; the meaning of the middle fields is open).
/// The text is code page 936; without the `cp936` feature Chinese text becomes U+FFFD.
pub fn read_statistics(bytes: &[u8]) -> Result<Vec<Vec<String>>, MlaserError> {
    check_size(bytes)?;
    let (text, _) = decode_text(bytes);
    let (lines, _, _) = split_lines(&text);
    Ok(lines.into_iter().filter(|l| !l.trim().is_empty()).map(|l| l.split(',').map(|f| f.to_string()).collect()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compensation_table_round_trip() {
        let src = "100,0.25\r\n200,0.5\r\n300,0.7";
        let t = CompensationTable::parse(src.as_bytes()).unwrap();
        assert_eq!(t.points.len(), 3);
        assert_eq!(t.points[1], CompPoint { speed_mm_s: 200.0, offset_mm: 0.5 });
        assert!(!t.trailing_newline);
        assert_eq!(t.to_text(), src);
        assert_eq!(t.to_scan_fly_str(), "100#0.25,200#0.5,300#0.7");
        let b = CompensationTable::from_scan_fly_str("100#0.25,200#0.5,300#0.75").unwrap();
        assert_eq!(t.differences(&b), vec![(300.0, Some(0.7), Some(0.75))]);
        assert_eq!(t.offset_at(150.0), Some(0.375));
        assert_eq!(t.offset_at(50.0), Some(0.25));
        assert_eq!(t.offset_at(900.0), Some(0.7));
        assert_eq!(t.offset_at(f64::NAN), None);
    }

    #[test]
    fn compensation_table_rejects_garbage() {
        for bad in ["", "100", "100,x", "a,b\r\n", "100,nan", "1e999,1", "100;0.3"] {
            assert!(CompensationTable::parse(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(CompensationTable::from_scan_fly_str("100#").is_err());
        assert!(CompensationTable::from_scan_fly_str("100-0.3").is_err());
        assert!(CompensationTable::parse(b"100,0.3\n\n200,0.4\n").is_ok());
    }

    #[test]
    fn ini_keeps_every_line_and_edits_in_place() {
        let src = "[IP]\r\nCardIP=192.0.2.10\r\nCardSubnet=255.255.255.0\r\nCardGateway=192.0.2.1\r\nCardPort=5000\r\n\r\nConnectWait=3\r\n; note\r\n[Soft]\r\nLang=1\r\n";
        let mut ini = IniFile::parse(src.as_bytes()).unwrap();
        assert_eq!(ini.to_bytes().unwrap(), src.as_bytes());
        assert_eq!(ini.sections(), ["IP", "Soft"]);
        assert_eq!(ini.get("ip", "cardport"), Some("5000"));
        let net = NetSettings::from_ini(&ini);
        assert_eq!(net.endpoints.len(), 1);
        assert_eq!(net.endpoints[0].port, Some(5000));
        assert_eq!(net.endpoints[0].gateway.as_deref(), Some("192.0.2.1"));
        assert_eq!(net.other, vec![("ConnectWait".to_string(), "3".to_string())]);
        assert_eq!(net.soft, vec![("Lang".to_string(), "1".to_string())]);
        ini.set("Soft", "Lang", "0");
        ini.set("Soft", "Skin", "2");
        ini.set("New", "A", "b");
        assert_eq!(ini.to_text(), src.replace("Lang=1\r\n", "Lang=0\r\nSkin=2\r\n[New]\r\nA=b\r\n"));
    }

    #[test]
    fn a_top_level_key_stays_at_the_top_level() {
        // No top-level pairs yet, and the file ends inside [Net]: the new key goes before the
        // first section, not into [Net].
        let src = "; settings\r\n[IP]\r\nCardPort=5000\r\n[Net]\r\nWait=3\r\n";
        let mut ini = IniFile::parse(src.as_bytes()).unwrap();
        ini.set("", "Foo", "1");
        assert_eq!(ini.get("", "Foo"), Some("1"));
        assert_eq!(ini.get("Net", "Foo"), None);
        assert_eq!(ini.to_text(), "; settings\r\nFoo=1\r\n[IP]\r\nCardPort=5000\r\n[Net]\r\nWait=3\r\n");
        // With a top-level pair already there, the next one follows it.
        ini.set("", "Bar", "2");
        assert_eq!(ini.to_text(), "; settings\r\nFoo=1\r\nBar=2\r\n[IP]\r\nCardPort=5000\r\n[Net]\r\nWait=3\r\n");
        // A file with no sections at all gets the key at the end.
        let mut flat = IniFile::parse(b"A=1\r\n").unwrap();
        flat.set("", "B", "2");
        assert_eq!(flat.to_text(), "A=1\r\nB=2\r\n");
        let mut empty = IniFile::parse(b"").unwrap();
        empty.set("", "C", "3");
        assert_eq!(empty.get("", "C"), Some("3"));
    }

    #[test]
    fn a_gbk_ini_read_without_its_code_page_is_not_written_back() {
        let bytes = b"[S]\r\nName=\xB2\xBB\xD0\xE2\xB8\xD6\r\n";
        let ini = IniFile::parse(bytes).unwrap();
        #[cfg(not(feature = "cp936"))]
        assert!(ini.to_bytes().is_err());
        #[cfg(feature = "cp936")]
        assert_eq!(ini.to_bytes().unwrap(), bytes.to_vec());
    }

    #[test]
    fn soft_para_positions() {
        let ini = IniFile::parse(b"[SC2000]\r\nNormalExit=0\r\nXAxis=250000\r\nYAxis=-1500\r\nZAxis=x\r\n").unwrap();
        let s = SoftPara::from_ini(&ini);
        assert_eq!(s.normal_exit, Some(false));
        assert_eq!(s.axes, [Some(250000), Some(-1500), None, None]);
        assert_eq!(s.axis_mm(0, 1000), Some(250.0));
        assert_eq!(s.axis_mm(1, 1000), Some(-1.5));
        assert_eq!(s.axis_mm(2, 1000), None);
        assert_eq!(s.axis_mm(9, 1000), None);
        assert_eq!(s.axis_mm(0, 0), None);
    }

    #[test]
    fn sc_files() {
        let src = "scFlie\r\n3\r\n7\r\n8\r\n9\r\neof\r\n";
        let f = ScFile::parse(src.as_bytes()).unwrap();
        assert_eq!(f.counted().unwrap(), ["7", "8", "9"]);
        assert_eq!(f.numbers(), vec![Some(3.0), Some(7.0), Some(8.0), Some(9.0)]);
        assert_eq!(f.to_text(), src);
        let g = ScFile::parse(b"scFlie\r\n4\r\n1\r\neof\r\n").unwrap();
        assert_eq!(g.counted(), None);
        assert!(ScFile::parse(b"scFlie\r\n1\r\n").is_err());
        assert!(ScFile::parse(b"hello\r\neof\r\n").is_err());
        assert!(ScFile::parse(b"").is_err());
        assert!(starts_with_sc_magic(b"scFlie\r\n"));
        assert!(!starts_with_sc_magic(b"<Param"));
    }

    #[test]
    fn statistics() {
        let rows = read_statistics(b"demo,10.0X20.0mm,0mm,0mm,1,5s,2026-01-01 08:00:00\n\n").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 7);
    }

    #[test]
    fn hostile_small_files_do_not_panic() {
        let big = vec![b'1'; MAX_SMALL_FILE_BYTES + 1];
        assert!(CompensationTable::parse(&big).is_err());
        assert!(IniFile::parse(&big).is_err());
        assert!(ScFile::parse(&big).is_err());
        for c in [&b"\xFF\xFE\x00"[..], b"\r", b"\n", b"\r\n\r\n", b"[", b"=", b"]=[", b"scFlie", b"scFlie\neof", b"1,2,3", b",", b"\xEF\xBB\xBF"] {
            let _ = CompensationTable::parse(c);
            let _ = IniFile::parse(c).map(|i| i.to_text());
            let _ = ScFile::parse(c).map(|s| (s.to_text(), s.counted().map(|v| v.len())));
            let _ = read_statistics(c);
        }
    }
}
