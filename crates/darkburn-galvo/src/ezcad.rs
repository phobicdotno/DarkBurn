//! EZCad configuration import (GALVO-18): field and lens-correction settings from a text
//! `markcfg`/ini file.
//!
//! EZCad's own configuration is not formally documented. This importer reads the common ini
//! style (`[section]`, `key = value`, `;` or `#` comments, UTF-8 or UTF-16 with BOM) and
//! recognises keys by a loose name match (case, spaces, underscores and dots ignored), see
//! [`import`]. A binary `MARKCFG` file is detected and refused with a clear message instead of
//! being guessed at. Which keys real EZCad files use is unconfirmed; see
//! `docs/hardware-checks-galvo.md`.

use std::fmt;

use crate::field::GalvoCal;
use crate::settings::{DeviceSettings, LaserSource};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The file looks binary.
    Binary,
    /// Not text in any supported encoding.
    NotText,
    /// Nothing recognisable was found.
    NothingFound,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Binary => write!(f, "This looks like a binary EZCad file. Only text/ini style configuration files can be imported."),
            Self::NotText => write!(f, "The file is not readable text."),
            Self::NothingFound => write!(f, "No field or lens correction settings were found in this file."),
        }
    }
}
impl std::error::Error for ImportError {}

/// What an import found.
#[derive(Clone, Debug, PartialEq)]
pub struct EzcadImport {
    /// Settings with the imported values applied on top of the defaults.
    pub settings: DeviceSettings,
    /// Path of the lens correction file named in the config, if any (not loaded here).
    pub cor_file: Option<String>,
    /// Normalised keys that were used.
    pub applied: Vec<String>,
    /// Keys that were not recognised.
    pub ignored: Vec<String>,
    pub warnings: Vec<String>,
}

fn normalise(k: &str) -> String {
    k.chars().filter(|c| !matches!(c, ' ' | '_' | '.' | '-' | '\t')).flat_map(|c| c.to_lowercase()).collect()
}

fn parse_f64(v: &str) -> Option<f64> {
    let t: String = v.trim().trim_matches('"').chars().take_while(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',' | 'e' | 'E')).collect();
    t.replace(',', ".").parse().ok()
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().trim_matches('"').to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn decode(bytes: &[u8]) -> Result<String, ImportError> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let le = bytes[0] == 0xFF;
        let units: Vec<u16> =
            bytes[2..].chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16(&units).map_err(|_| ImportError::NotText);
    }
    let head = &bytes[..bytes.len().min(512)];
    if head.contains(&0) {
        return Err(ImportError::Binary);
    }
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    Ok(String::from_utf8_lossy(text).into_owned())
}

/// Imports field, calibration, orientation, lens file name, laser type and timings.
///
/// Recognised keys (after normalising): `fieldsize` / `lenssize` / `workarea` (mm),
/// `offsetx` / `offsety`, `galvo1scale|skew|bulge|trapezoid|negate|isx` and the same for
/// `galvo2`, `scalex` / `scaley` (galvo 1 / 2 scale), `flipx`, `flipy`, `swapxy`, `rotate`
/// (degrees), `corfile` / `correctionfile` / `lensfile`, `lasertype` (`fiber`, `mopa`, `co2`,
/// `uv`), `jumpspeed`, `minjumpdelay`, `maxjumpdelay`, `jumpdistancelimit`, `laserontc`,
/// `laserofftc`, `endtc`, `polygontc`.
pub fn import(bytes: &[u8]) -> Result<EzcadImport, ImportError> {
    let text = decode(bytes)?;
    let mut out = EzcadImport { settings: DeviceSettings::default(), cor_file: None, applied: Vec::new(), ignored: Vec::new(), warnings: Vec::new() };
    let mut g1 = GalvoCal::identity(true);
    let mut g2 = GalvoCal::identity(false);
    let s = &mut out.settings;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with([';', '#', '[']) {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = normalise(k);
        let f = parse_f64(v);
        let b = parse_bool(v);
        let mut used = true;
        match (key.as_str(), f, b) {
            ("fieldsize" | "lenssize" | "workarea" | "lensfieldsize", Some(x), _) if x > 0.0 => s.field.size_mm = x,
            ("offsetx" | "fieldoffsetx", Some(x), _) => s.field.offset_mm.0 = x,
            ("offsety" | "fieldoffsety", Some(x), _) => s.field.offset_mm.1 = x,
            ("galvo1scale" | "scalex", Some(x), _) if x > 0.0 => g1.scale = if x > 5.0 { x / 100.0 } else { x },
            ("galvo2scale" | "scaley", Some(x), _) if x > 0.0 => g2.scale = if x > 5.0 { x / 100.0 } else { x },
            ("galvo1skew", Some(x), _) => g1.skew = x,
            ("galvo2skew", Some(x), _) => g2.skew = x,
            ("galvo1bulge", Some(x), _) => g1.bulge = x,
            ("galvo2bulge", Some(x), _) => g2.bulge = x,
            ("galvo1trapezoid", Some(x), _) => g1.trapezoid = x,
            ("galvo2trapezoid", Some(x), _) => g2.trapezoid = x,
            ("galvo1negate", _, Some(x)) => g1.negate = x,
            ("galvo2negate", _, Some(x)) => g2.negate = x,
            ("galvo1isx", _, Some(x)) => g1.is_x = x,
            ("galvo2isx", _, Some(x)) => g2.is_x = x,
            ("flipx", _, Some(x)) => s.field.flip_x = x,
            ("flipy", _, Some(x)) => s.field.flip_y = x,
            ("swapxy", _, Some(x)) => s.field.swap_xy = x,
            ("rotate", Some(x), _) => s.field.rotate_quarters = (((x / 90.0).round() as i64).rem_euclid(4)) as u8,
            ("corfile" | "correctionfile" | "lensfile", _, _) => {
                let p = v.trim().trim_matches('"');
                if p.is_empty() {
                    used = false;
                } else {
                    out.cor_file = Some(p.to_string());
                }
            }
            ("lasertype", _, _) => match v.trim().trim_matches('"').to_lowercase().as_str() {
                "fiber" | "fibre" => s.source = LaserSource::Fiber,
                "mopa" => s.source = LaserSource::Mopa,
                "co2" => s.source = LaserSource::Co2,
                "uv" => s.source = LaserSource::Uv,
                other => {
                    out.warnings.push(format!("Unknown laser type '{other}'."));
                    used = false;
                }
            },
            ("jumpspeed", Some(x), _) if x > 0.0 => s.timings.jump_speed_mm_s = x,
            ("minjumpdelay", Some(x), _) => s.timings.min_jump_delay = x as i32,
            ("maxjumpdelay", Some(x), _) => s.timings.max_jump_delay = x as i32,
            ("jumpdistancelimit" | "jumpdistlimit", Some(x), _) => s.timings.jump_distance_limit_mm = x,
            ("laserontc" | "laserondelay", Some(x), _) => s.timings.laser_on_tc = x as i32,
            ("laserofftc" | "laseroffdelay", Some(x), _) => s.timings.laser_off_tc = x as i32,
            ("endtc" | "enddelay", Some(x), _) => s.timings.end_tc = x as i32,
            ("polygontc" | "polygondelay", Some(x), _) => s.timings.polygon_tc = x as i32,
            _ => used = false,
        }
        if used {
            out.applied.push(key);
        } else {
            out.ignored.push(key);
        }
    }
    s.field.galvo1 = g1;
    s.field.galvo2 = g2;
    if out.applied.is_empty() {
        return Err(ImportError::NothingFound);
    }
    if g1.is_x == g2.is_x {
        out.warnings.push("Both galvos are set to the same axis.".into());
    }
    Ok(out)
}

impl EzcadImport {
    /// Copies the imported field, orientation, source and timing values into existing settings.
    pub fn apply_to(&self, dev: &mut DeviceSettings) {
        dev.field = self.settings.field.clone();
        dev.source = self.settings.source;
        if self.applied.iter().any(|k| k.contains("jump") || k.ends_with("tc") || k.ends_with("delay")) {
            dev.timings = self.settings.timings;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; EZCad style config\n[Laser]\nLaser Type = MOPA\n[Lens]\nField_Size = 110,5\nOffsetX = 1.5\nOffsetY=-2\nGalvo1.Scale = 101.5\nGalvo2.Scale = 0.995\nGalvo1 Skew = 0.002\nGalvo1.Bulge = 0.01\nGalvo2.Trapezoid = -0.004\nGalvo2.Negate = 1\nGalvo2.IsX = false\nFlipY = yes\nRotate = 270\nCorFile = \"C:\\\\lens\\\\f160.cor\"\nJump Speed = 4000\nLaserOnTC = 120\nEndTC=250\nSomethingElse = 5\n";

    #[test]
    fn imports_field_calibration_and_timings() {
        let r = import(SAMPLE.as_bytes()).unwrap();
        let s = &r.settings;
        assert_eq!(s.field.size_mm, 110.5);
        assert_eq!(s.field.offset_mm, (1.5, -2.0));
        assert!((s.field.galvo1.scale - 1.015).abs() < 1e-12);
        assert_eq!(s.field.galvo2.scale, 0.995);
        assert_eq!(s.field.galvo1.skew, 0.002);
        assert_eq!(s.field.galvo1.bulge, 0.01);
        assert_eq!(s.field.galvo2.trapezoid, -0.004);
        assert!(s.field.galvo2.negate && !s.field.galvo2.is_x);
        assert!(s.field.flip_y && !s.field.flip_x);
        assert_eq!(s.field.rotate_quarters, 3);
        assert_eq!(s.source, LaserSource::Mopa);
        assert_eq!(s.timings.jump_speed_mm_s, 4000.0);
        assert_eq!((s.timings.laser_on_tc, s.timings.end_tc), (120, 250));
        assert_eq!(r.cor_file.as_deref(), Some("C:\\\\lens\\\\f160.cor"));
        assert_eq!(r.ignored, vec!["somethingelse".to_string()]);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn utf16_with_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "FieldSize=80\r\nOffsetX=3\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let r = import(&bytes).unwrap();
        assert_eq!(r.settings.field.size_mm, 80.0);
        assert_eq!(r.settings.field.offset_mm.0, 3.0);
    }

    #[test]
    fn binary_and_empty_files_are_refused() {
        assert_eq!(import(&[0, 1, 2, 3, 0, 0, 9, 9]), Err(ImportError::Binary));
        assert_eq!(import(b"nothing = here\n"), Err(ImportError::NothingFound));
        assert_eq!(import(b""), Err(ImportError::NothingFound));
    }

    #[test]
    fn bad_values_are_ignored_not_fatal() {
        let r = import(b"FieldSize=abc\nOffsetX=4\nLaserType=plasma\n").unwrap();
        assert_eq!(r.settings.field.size_mm, DeviceSettings::default().field.size_mm);
        assert_eq!(r.settings.field.offset_mm.0, 4.0);
        assert_eq!(r.warnings.len(), 1);
        assert!(r.ignored.contains(&"fieldsize".to_string()));
    }

    #[test]
    fn apply_keeps_other_settings() {
        let r = import(b"FieldSize=70\nJumpSpeed=1234\n").unwrap();
        let mut dev = DeviceSettings { require_framing: true, ..Default::default() };
        r.apply_to(&mut dev);
        assert_eq!(dev.field.size_mm, 70.0);
        assert_eq!(dev.timings.jump_speed_mm_s, 1234.0);
        assert!(dev.require_framing);
        let only_field = import(b"FieldSize=70\n").unwrap();
        let mut d2 = DeviceSettings::default();
        d2.timings.end_tc = 999;
        only_field.apply_to(&mut d2);
        assert_eq!(d2.timings.end_tc, 999);
    }
}
