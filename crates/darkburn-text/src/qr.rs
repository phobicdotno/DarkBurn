//! QR codes: editable data model (text, URL, Wi-Fi, MECARD contact) that regenerates its geometry.
//!
//! Geometry is in mm, Y up. The symbol occupies `[0, size] x [0, size]` with `size = modules * module_mm`;
//! the quiet zone is reported in [`QrResult::total_size_mm`] but not drawn.

use crate::error::{Result, TextError};
use crate::geomutil;
use kurbo::{BezPath, Rect, Shape};
use qrcode::{EcLevel, QrCode};
use serde::{Deserialize, Serialize};

/// Most bytes a code may carry (limit shown to the user, below the QR format maximum).
pub const MAX_QR_BYTES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EccLevel {
    /// ~7% recovery
    L,
    /// ~15%
    #[default]
    M,
    /// ~25%
    Q,
    /// ~30%
    H,
}

impl EccLevel {
    fn to_ec(self) -> EcLevel {
        match self {
            EccLevel::L => EcLevel::L,
            EccLevel::M => EcLevel::M,
            EccLevel::Q => EcLevel::Q,
            EccLevel::H => EcLevel::H,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WifiSecurity {
    #[default]
    Wpa,
    Wep,
    Open,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContactCard {
    /// Written as `Last,First` by the caller if wanted; used verbatim after escaping.
    pub name: String,
    pub phone: String,
    pub email: String,
    pub url: String,
    pub address: String,
    pub note: String,
    pub birthday: String,
    pub nickname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QrContent {
    Text(String),
    Url(String),
    Wifi { ssid: String, password: String, security: WifiSecurity, hidden: bool },
    Contact(ContactCard),
}

fn esc(s: &str, chars: &[char]) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\\' || chars.contains(&c) {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

impl QrContent {
    /// The string stored in the code. `eval` is applied to every user field first (variable text).
    pub fn payload_with(&self, eval: &dyn Fn(&str) -> String) -> String {
        match self {
            QrContent::Text(t) => eval(t),
            QrContent::Url(u) => {
                let u = eval(u).trim().to_string();
                if u.is_empty() || u.contains("://") || u.starts_with("mailto:") || u.starts_with("tel:") { u } else { format!("https://{u}") }
            }
            QrContent::Wifi { ssid, password, security, hidden } => {
                let t = match security {
                    WifiSecurity::Wpa => "WPA",
                    WifiSecurity::Wep => "WEP",
                    WifiSecurity::Open => "nopass",
                };
                let special = ['\\', ';', ',', ':', '"'];
                let mut s = format!("WIFI:T:{t};S:{};", esc(&eval(ssid), &special));
                if *security != WifiSecurity::Open {
                    s.push_str(&format!("P:{};", esc(&eval(password), &special)));
                }
                if *hidden {
                    s.push_str("H:true;");
                }
                s.push(';');
                s
            }
            QrContent::Contact(c) => {
                let special = ['\\', ';', ',', ':'];
                let mut s = String::from("MECARD:");
                let mut field = |k: &str, v: &str| {
                    let v = eval(v);
                    if !v.is_empty() {
                        s.push_str(&format!("{k}:{};", esc(&v, &special)));
                    }
                };
                field("N", &c.name);
                field("TEL", &c.phone);
                field("EMAIL", &c.email);
                field("URL", &c.url);
                field("ADR", &c.address);
                field("BDAY", &c.birthday);
                field("NICKNAME", &c.nickname);
                field("NOTE", &c.note);
                s.push(';');
                s
            }
        }
    }

    /// Payload without variable text.
    pub fn payload(&self) -> String {
        self.payload_with(&|s| s.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QrSpec {
    pub content: QrContent,
    pub ecc: EccLevel,
    /// Side of one module in mm.
    pub module_mm: f64,
    /// Quiet zone in modules (only counted in the total size).
    pub quiet_zone: u32,
    /// Merge touching modules into one outline (fewer, cleaner paths); otherwise one square per module.
    pub merged: bool,
}

impl Default for QrSpec {
    fn default() -> Self {
        Self { content: QrContent::Text(String::new()), ecc: EccLevel::M, module_mm: 1.0, quiet_zone: 4, merged: true }
    }
}

#[derive(Clone, Debug)]
pub struct QrResult {
    /// The string that was encoded.
    pub payload: String,
    /// Modules per side.
    pub modules: usize,
    /// Side of the symbol in mm (without quiet zone).
    pub size_mm: f64,
    /// Side including the quiet zone.
    pub total_size_mm: f64,
    /// Dark modules, row-major, row 0 at the top of the symbol.
    pub matrix: Vec<Vec<bool>>,
    /// Geometry: one square per module, or merged outlines when `merged`.
    pub paths: Vec<BezPath>,
}

impl QrSpec {
    /// Generate with fixed content.
    pub fn generate(&self) -> Result<QrResult> {
        self.generate_with(&|s| s.to_string())
    }

    /// Generate after evaluating variable text in every user field with `eval`
    /// (for example `|s| state.evaluate(s, &ctx)`).
    pub fn generate_with(&self, eval: &dyn Fn(&str) -> String) -> Result<QrResult> {
        let payload = self.content.payload_with(eval);
        if payload.is_empty() {
            return Err(TextError::Qr("nothing to encode".into()));
        }
        if payload.len() > MAX_QR_BYTES {
            return Err(TextError::Qr(format!("{} bytes is over the {MAX_QR_BYTES} byte limit", payload.len())));
        }
        if !(self.module_mm.is_finite() && self.module_mm > 0.0) {
            return Err(TextError::Qr("module size must be positive".into()));
        }
        let code = QrCode::with_error_correction_level(payload.as_bytes(), self.ecc.to_ec()).map_err(|e| TextError::Qr(e.to_string()))?;
        let n = code.width();
        let colors = code.to_colors();
        let matrix: Vec<Vec<bool>> = (0..n).map(|y| (0..n).map(|x| colors[y * n + x] == qrcode::Color::Dark).collect()).collect();
        let paths = matrix_paths(&matrix, self.module_mm, self.merged);
        let size_mm = n as f64 * self.module_mm;
        Ok(QrResult { payload, modules: n, size_mm, total_size_mm: size_mm + 2.0 * self.quiet_zone as f64 * self.module_mm, matrix, paths })
    }
}

/// Turn a dark-module matrix (row 0 on top) into geometry in mm, Y up.
pub fn matrix_paths(matrix: &[Vec<bool>], module_mm: f64, merged: bool) -> Vec<BezPath> {
    let h = matrix.len();
    let mut rects: Vec<Rect> = Vec::new();
    for (row, line) in matrix.iter().enumerate() {
        for (col, dark) in line.iter().enumerate() {
            if *dark {
                let x = col as f64 * module_mm;
                let y = (h - 1 - row) as f64 * module_mm;
                rects.push(Rect::new(x, y, x + module_mm, y + module_mm));
            }
        }
    }
    if !merged {
        return rects.iter().map(|r| r.to_path(0.0)).collect();
    }
    let paths: Vec<BezPath> = rects.iter().map(|r| r.to_path(0.0)).collect();
    geomutil::weld(&paths, 0.01)
}
