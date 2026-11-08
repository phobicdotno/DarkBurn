//! Firmware guess from banner text (DEV-02 logic only).
//!
//! The application reads whatever a transport prints after connecting (and the reply to a
//! harmless identification line such as `$I`, `version` or `M115`) and passes it here. This
//! module never touches a port.

use darkburn_grbl::protocol::parse_banner;
use darkburn_grbl::{FirmwareInfo, FirmwareKind};
use serde::{Deserialize, Serialize};

use crate::machine::MachineFamily;

/// Which firmware a banner looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FirmwareGuess {
    Grbl,
    GrblHal,
    FluidNc,
    Smoothie,
    Marlin,
    Snapmaker,
    Unknown,
}

impl FirmwareGuess {
    pub fn family(self) -> Option<MachineFamily> {
        Some(match self {
            Self::Grbl => MachineFamily::Grbl,
            Self::GrblHal => MachineFamily::GrblHal,
            Self::FluidNc => MachineFamily::FluidNc,
            Self::Smoothie => MachineFamily::Smoothie,
            Self::Marlin => MachineFamily::Marlin,
            Self::Snapmaker => MachineFamily::Snapmaker,
            Self::Unknown => return None,
        })
    }
}

/// What [`identify`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectResult {
    pub firmware: FirmwareGuess,
    /// Version text when the banner has one.
    pub version: Option<String>,
    /// The banner line that decided it.
    pub evidence: String,
    /// Baud rates worth trying, most likely first.
    pub baud_candidates: Vec<u32>,
    /// The identification line to send next when the banner alone is not enough.
    pub next_probe: Option<&'static str>,
}

/// Baud rates to try for a firmware, most likely first. These are the documented defaults
/// followed by the other rates seen on laser boards.
pub fn baud_candidates(fw: FirmwareGuess) -> Vec<u32> {
    match fw {
        FirmwareGuess::Grbl | FirmwareGuess::GrblHal | FirmwareGuess::FluidNc => vec![115_200, 230_400, 250_000, 57_600, 19_200, 9_600],
        // Smoothieware talks over USB CDC, where the rate is ignored; UART builds use 115200.
        FirmwareGuess::Smoothie => vec![115_200, 250_000, 57_600, 9_600],
        FirmwareGuess::Marlin | FirmwareGuess::Snapmaker => vec![115_200, 250_000, 500_000, 57_600, 38_400, 19_200, 9_600],
        FirmwareGuess::Unknown => vec![115_200, 250_000, 57_600, 230_400, 38_400, 19_200, 9_600, 500_000],
    }
}

/// Identification lines to try, in order, when a plain connect printed nothing recognisable.
/// `$I` is GRBL's build info; `version` is a Smoothieware console command; `M115` reports
/// `FIRMWARE_NAME` on Marlin-style firmware.
pub const PROBE_LINES: [&str; 3] = ["$I", "version", "M115"];

/// Guesses the firmware from one banner or reply line.
pub fn identify(line: &str) -> DetectResult {
    let t = line.trim();
    let low = t.to_ascii_lowercase();
    let mk = |fw: FirmwareGuess, version: Option<String>| DetectResult {
        firmware: fw,
        version,
        evidence: t.to_owned(),
        baud_candidates: baud_candidates(fw),
        next_probe: None,
    };

    if let Some(info) = parse_banner(t) {
        let fw = match info.kind {
            FirmwareKind::Grbl => FirmwareGuess::Grbl,
            FirmwareKind::GrblHal => FirmwareGuess::GrblHal,
            FirmwareKind::FluidNc => FirmwareGuess::FluidNc,
        };
        return mk(fw, Some(version_of(&info)));
    }
    if low.contains("fluidnc") {
        return mk(FirmwareGuess::FluidNc, token_after(t, "FluidNC"));
    }
    if low.contains("grblhal") || low.contains("[firmware:grblhal") {
        return mk(FirmwareGuess::GrblHal, None);
    }
    if low.starts_with("[ver:") || low.starts_with("[opt:") || low.starts_with("[msg:") {
        return mk(FirmwareGuess::Grbl, None);
    }
    if low.contains("snapmaker") {
        return mk(FirmwareGuess::Snapmaker, field_after(t, "SOURCE_CODE_URL").or_else(|| token_after(t, "Snapmaker")));
    }
    if low.contains("firmware_name:") {
        // M115 reply: `FIRMWARE_NAME:Marlin 2.1.2 (...) SOURCE_CODE_URL:...`
        let name = field_after(t, "FIRMWARE_NAME").unwrap_or_default();
        let nl = name.to_ascii_lowercase();
        if nl.starts_with("smoothie") {
            return mk(FirmwareGuess::Smoothie, name.split_whitespace().nth(1).map(str::to_owned));
        }
        if nl.contains("marlin") {
            return mk(FirmwareGuess::Marlin, name.split_whitespace().nth(1).map(str::to_owned));
        }
        // Another Marlin-protocol firmware (Klipper, RepRapFirmware...): the closest dialect.
        return mk(FirmwareGuess::Marlin, name.split_whitespace().nth(1).map(str::to_owned));
    }
    if low.starts_with("smoothie") || low.contains("smoothieware") || (low.contains("build version:") && low.contains("mcu:")) {
        return mk(FirmwareGuess::Smoothie, field_after(t, "Build version"));
    }
    if low.contains("marlin") {
        return mk(FirmwareGuess::Marlin, token_after(t, "Marlin"));
    }
    DetectResult {
        firmware: FirmwareGuess::Unknown,
        version: None,
        evidence: t.to_owned(),
        baud_candidates: baud_candidates(FirmwareGuess::Unknown),
        next_probe: Some(PROBE_LINES[0]),
    }
}

/// Guesses from everything received so far: the first line that identifies a firmware wins;
/// a bare `start` or `ok` is a weak hint for Marlin-style boards and picks the next probe.
pub fn identify_lines<S: AsRef<str>>(lines: &[S]) -> DetectResult {
    let mut best: Option<DetectResult> = None;
    for l in lines {
        let r = identify(l.as_ref());
        if r.firmware == FirmwareGuess::Unknown {
            continue;
        }
        match &best {
            None => best = Some(r),
            // A later line of the same firmware may carry the version the banner lacked.
            Some(b) if b.firmware == r.firmware && b.version.is_none() && r.version.is_some() => best = Some(r),
            _ => {}
        }
    }
    if let Some(b) = best {
        return b;
    }
    let saw = |s: &str| lines.iter().any(|l| l.as_ref().trim().eq_ignore_ascii_case(s));
    let next = if saw("start") {
        Some("M115")
    } else if saw("ok") {
        Some("version")
    } else {
        Some(PROBE_LINES[0])
    };
    DetectResult {
        firmware: FirmwareGuess::Unknown,
        version: None,
        evidence: lines.last().map(|l| l.as_ref().trim().to_owned()).unwrap_or_default(),
        baud_candidates: baud_candidates(FirmwareGuess::Unknown),
        next_probe: next,
    }
}

fn version_of(info: &FirmwareInfo) -> String {
    if info.kind == FirmwareKind::FluidNc && !info.build.is_empty() { info.build.clone() } else { info.version.clone() }
}

/// The word after `key` (`Marlin 2.1.2` -> `2.1.2`).
fn token_after(text: &str, key: &str) -> Option<String> {
    let i = text.to_ascii_lowercase().find(&key.to_ascii_lowercase())?;
    text[i + key.len()..].split_whitespace().next().map(|s| s.trim_matches(|c| c == ',' || c == ':').to_owned()).filter(|s| !s.is_empty())
}

/// The value of `KEY:value` up to the next comma or space-keyword.
fn field_after(text: &str, key: &str) -> Option<String> {
    let i = text.find(key)?;
    let rest = text[i + key.len()..].trim_start_matches(':').trim_start();
    // `FIRMWARE_NAME:Marlin 2.1.2 (Github) SOURCE_CODE_URL:...` -> keep up to the next KEY:
    let end = rest.find(" SOURCE_CODE_URL").or_else(|| rest.find(", ")).unwrap_or(rest.len());
    let v = rest[..end].trim();
    if v.is_empty() { None } else { Some(v.to_owned()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grbl_family_banners() {
        assert_eq!(identify("Grbl 1.1h ['$' for help]").firmware, FirmwareGuess::Grbl);
        assert_eq!(identify("GrblHAL 1.1f ['$' or '$HELP' for help]").firmware, FirmwareGuess::GrblHal);
        let r = identify("Grbl 3.9 [FluidNC v3.9.7 (noradio) '$' for help]");
        assert_eq!(r.firmware, FirmwareGuess::FluidNc);
        assert_eq!(r.baud_candidates[0], 115_200);
        assert_eq!(identify("[VER:1.1h.20190825:]").firmware, FirmwareGuess::Grbl);
    }

    #[test]
    fn smoothie_banners() {
        assert_eq!(identify("Smoothie").firmware, FirmwareGuess::Smoothie);
        let r = identify("Build version: edge-94de3d9, Build date: Oct 28 2014 13:24:47, MCU: LPC1769, System Clock: 120MHz");
        assert_eq!(r.firmware, FirmwareGuess::Smoothie);
        assert_eq!(r.version.as_deref(), Some("edge-94de3d9"));
    }

    #[test]
    fn marlin_and_snapmaker() {
        let r = identify("FIRMWARE_NAME:Marlin 2.1.2 (Github) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0");
        assert_eq!(r.firmware, FirmwareGuess::Marlin);
        assert_eq!(r.version.as_deref(), Some("2.1.2"));
        assert!(r.baud_candidates.contains(&250_000));
        assert_eq!(identify("echo:Marlin 1.1.9").firmware, FirmwareGuess::Marlin);
        assert_eq!(identify("FIRMWARE_NAME:Snapmaker 2.0 MACHINE_TYPE:A350").firmware, FirmwareGuess::Snapmaker);
    }

    #[test]
    fn unknown_suggests_a_probe() {
        let r = identify("hello");
        assert_eq!(r.firmware, FirmwareGuess::Unknown);
        assert_eq!(r.next_probe, Some("$I"));
        let r = identify_lines(&["start", "ok"]);
        assert_eq!(r.next_probe, Some("M115"));
        let r = identify_lines(&["", "Smoothie", "ok"]);
        assert_eq!(r.firmware, FirmwareGuess::Smoothie);
    }

    #[test]
    fn family_mapping() {
        assert_eq!(FirmwareGuess::Marlin.family(), Some(MachineFamily::Marlin));
        assert_eq!(FirmwareGuess::Unknown.family(), None);
    }
}
