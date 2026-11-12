//! Recognising common diode lasers from what their controller prints.
//!
//! The rules come from console output that owners have posted publicly (forum threads and vendor
//! help pages), never from firmware source:
//!
//! * Ortur boards answer `$I` with tags such as `[ORIGIN: China]`, `[PRODUCER: ORTUR]`,
//!   `[AUTHOR: ORTUR]`, `[MODEL: Ortur Laser Master 2 Pro S1]` and `[OLF: 172]` (their firmware
//!   number), and greet with `Ortur Laser Master 2 Ready!`.
//! * xTool D1 boards print `[xTool D1Pro:ver 40.31.002.01 B5]` or `[XTool D1:ver 40.30.003.01 B9]`.
//! * ESP32 boards built from Grbl_ESP32 or FluidNC print `[MSG:Using machine:<name>]`, and the
//!   name often carries the brand (Atomstack, Sculpfun and others).
//!
//! Anything else is matched by brand name wherever it appears in the welcome text or the `$I`
//! reply. A machine that never prints its name cannot be told apart from plain GRBL; the caller
//! then still has the controller's own `$130`/`$131`/`$30` values to size the device.

/// A laser recognised from controller text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedLaser {
    /// The maker, e.g. `Ortur`.
    pub brand: &'static str,
    /// The model as the controller names it, e.g. `Ortur Laser Master 2 Pro S1`. May be just the
    /// brand when the controller says no more.
    pub model: String,
    /// Vendor firmware version when printed, e.g. `OLF 172` or `40.31.002.01 B5`.
    pub firmware: String,
}

impl DetectedLaser {
    /// One line for the user, e.g. `Ortur Laser Master 2 Pro S1 (OLF 172)`.
    pub fn label(&self) -> String {
        if self.firmware.is_empty() { self.model.clone() } else { format!("{} ({})", self.model, self.firmware) }
    }
}

/// Brand names looked for, lower case, with how they are written.
const BRANDS: [(&str, &str); 9] = [
    ("xtool", "xTool"),
    ("ortur", "Ortur"),
    ("aufero", "Ortur"),
    ("atomstack", "Atomstack"),
    ("sculpfun", "Sculpfun"),
    ("neje", "NEJE"),
    ("twotrees", "TwoTrees"),
    ("longer", "Longer"),
    ("creality", "Creality"),
];

fn brand_in(text: &str) -> Option<(&'static str, usize)> {
    let lower = text.to_ascii_lowercase();
    BRANDS.iter().filter_map(|(key, name)| lower.find(key).map(|i| (*name, i))).min_by_key(|(_, i)| *i)
}

/// `[TAG: value]` or `[TAG:value]` split into its two parts.
fn tag(line: &str) -> Option<(&str, &str)> {
    let body = line.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (t, v) = body.split_once(':')?;
    Some((t.trim(), v.trim()))
}

fn tidy(model: &str) -> String {
    model.replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Looks through welcome text and `$I` output (any number of lines) for a known laser.
pub fn identify_laser(text: &str) -> Option<DetectedLaser> {
    let mut producer = None;
    let mut model = None;
    let mut firmware = String::new();
    let mut loose: Option<DetectedLaser> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some((t, v)) = tag(line) {
            let tl = t.to_ascii_lowercase();
            // [xTool D1Pro:ver 40.31.002.01 B5]
            if tl.starts_with("xtool") {
                let name = t.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
                let fw = v.strip_prefix("ver").map(str::trim).unwrap_or(v);
                return Some(DetectedLaser { brand: "xTool", model: tidy(&format!("xTool {name}")), firmware: fw.to_owned() });
            }
            match tl.as_str() {
                "producer" => producer = brand_in(v).map(|b| b.0),
                "model" => model = Some(v.to_owned()),
                "olf" => firmware = format!("OLF {v}"),
                // [MSG:Using machine:SCULPFUN_S30]
                "msg" => {
                    if let Some(name) = v.strip_prefix("Using machine:")
                        && let Some((brand, _)) = brand_in(name)
                    {
                        loose = Some(DetectedLaser { brand, model: tidy(name), firmware: String::new() });
                    }
                }
                _ => {}
            }
            continue;
        }
        // "Ortur Laser Master 2 Ready!"
        if let Some(name) = line.strip_suffix("Ready!")
            && let Some((brand, _)) = brand_in(name)
            && loose.is_none()
        {
            loose = Some(DetectedLaser { brand, model: tidy(name), firmware: String::new() });
        }
    }
    if let Some(m) = model
        && let Some(brand) = brand_in(&m).map(|b| b.0).or(producer)
    {
        return Some(DetectedLaser { brand, model: tidy(&m), firmware });
    }
    if let Some(mut l) = loose {
        if l.firmware.is_empty() {
            l.firmware = firmware;
        }
        return Some(l);
    }
    if let Some(brand) = producer {
        return Some(DetectedLaser { brand, model: brand.to_owned(), firmware });
    }
    // Last resort: the brand name anywhere (a `[VER:...]` tail, a custom banner).
    let (brand, at) = brand_in(text)?;
    let rest = &text[at..];
    let end = rest.find([']', '\n', '\r', '[', '\'']).unwrap_or(rest.len());
    Some(DetectedLaser { brand, model: tidy(&rest[..end]), firmware })
}

/// Collects controller lines as they arrive and keeps the best answer so far.
#[derive(Clone, Debug, Default)]
pub struct LaserDetector {
    text: String,
    lines: usize,
}

impl LaserDetector {
    /// Only the start of a session is looked at: the welcome text and the `$I` reply.
    pub const MAX_LINES: usize = 200;

    pub fn feed(&mut self, line: &str) {
        // Status reports and plain acknowledgements say nothing about the machine.
        if self.lines >= Self::MAX_LINES || line.starts_with('<') || line == "ok" {
            return;
        }
        self.lines += 1;
        self.text.push_str(line);
        self.text.push('\n');
    }
    pub fn result(&self) -> Option<DetectedLaser> {
        identify_laser(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ortur_tags_and_welcome() {
        let text = "Grbl 1.1f ['$' for help]\n[ORIGIN: China]\n[PRODUCER: ORTUR]\n[AUTHOR: ORTUR]\n[MODEL: Ortur Laser Master 2 Pro S1]\n[OLF: 172]\n[VER:1.1f.20210821:]\n[OPT:VZ,15,254]";
        let d = identify_laser(text).unwrap();
        assert_eq!((d.brand, d.model.as_str(), d.firmware.as_str()), ("Ortur", "Ortur Laser Master 2 Pro S1", "OLF 172"));
        assert_eq!(d.label(), "Ortur Laser Master 2 Pro S1 (OLF 172)");
        let d = identify_laser("Ortur Laser Master 2 Ready!\nGrbl 1.1h ['$' for help]").unwrap();
        assert_eq!((d.brand, d.model.as_str()), ("Ortur", "Ortur Laser Master 2"));
        // Aufero is an Ortur brand.
        assert_eq!(identify_laser("[PRODUCER: ORTUR]\n[MODEL: Aufero Laser 2]").unwrap().brand, "Ortur");
    }

    #[test]
    fn xtool_version_lines() {
        let d = identify_laser("ok\n[xTool D1Pro:ver 40.31.002.01 B5]").unwrap();
        assert_eq!((d.brand, d.model.as_str(), d.firmware.as_str()), ("xTool", "xTool D1Pro", "40.31.002.01 B5"));
        let d = identify_laser("$I\nok\nok\n[XTool D1:ver 40.30.003.01 B9]").unwrap();
        assert_eq!((d.brand, d.model.as_str()), ("xTool", "xTool D1"));
    }

    #[test]
    fn machine_names_and_brand_anywhere() {
        let d = identify_laser("[MSG:Using machine:SCULPFUN_S30]\nGrbl 1.1f ['$' for help]").unwrap();
        assert_eq!((d.brand, d.model.as_str()), ("Sculpfun", "SCULPFUN S30"));
        let d = identify_laser("Grbl 1.1f ['$' for help]\n[VER:1.1f.20220810:ATOMSTACK A20]\n[OPT:V,15,128]").unwrap();
        assert_eq!((d.brand, d.model.as_str()), ("Atomstack", "ATOMSTACK A20"));
    }

    #[test]
    fn plain_grbl_is_not_a_known_laser() {
        assert_eq!(identify_laser("Grbl 1.1h ['$' for help]\n[VER:1.1h.20190825:]\n[OPT:V,15,128]\n[MSG:Using cpu_map...esp32-s3]"), None);
        assert_eq!(identify_laser(""), None);
        let mut det = LaserDetector::default();
        det.feed("<Idle|MPos:0,0,0|FS:0,0>");
        det.feed("ok");
        assert_eq!(det.result(), None);
        det.feed("[MODEL: Ortur Laser Master 3]");
        det.feed("[PRODUCER: ORTUR]");
        assert_eq!(det.result().unwrap().model, "Ortur Laser Master 3");
    }
}
