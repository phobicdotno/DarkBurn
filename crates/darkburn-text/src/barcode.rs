//! Barcodes: Code 128 (sets B and C), Code 39, EAN-13, UPC-A, Interleaved 2 of 5, and Data Matrix
//! (through the permissively licensed `datamatrix` crate). Encoders are written from the public
//! symbology specifications; output is rectangles in mm, Y up, origin at the bottom-left of the bars.
//!
//! Gaps: Code 128 set A / FNC characters, GS1-128, EAN-8, Code 93, PDF417 and Aztec are not implemented.
//! No human-readable text line is produced (add a text object under the barcode).

use crate::error::{Result, TextError};
use crate::geomutil;
use crate::qr::matrix_paths;
use kurbo::{BezPath, Rect, Shape};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarcodeKind {
    Code128,
    Code39,
    Ean13,
    UpcA,
    Itf,
    DataMatrix,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarcodeSpec {
    pub kind: BarcodeKind,
    pub data: String,
    /// Narrow bar width (1D) or module size (Data Matrix), mm.
    pub module_mm: f64,
    /// Bar height in mm (1D codes only).
    pub height_mm: f64,
    /// Quiet zone in modules each side (only counted in the total width).
    pub quiet_zone: u32,
    /// Wide/narrow ratio for Code 39 and ITF.
    pub wide_ratio: f64,
    /// Append the optional mod-43 check character to Code 39.
    pub code39_check: bool,
}

impl Default for BarcodeSpec {
    fn default() -> Self {
        Self { kind: BarcodeKind::Code128, data: String::new(), module_mm: 0.4, height_mm: 15.0, quiet_zone: 10, wide_ratio: 3.0, code39_check: false }
    }
}

#[derive(Clone, Debug)]
pub struct BarcodeResult {
    /// What was encoded (with check digit / padding added).
    pub payload: String,
    /// Width of the symbol in mm, without quiet zones.
    pub width_mm: f64,
    pub height_mm: f64,
    pub total_width_mm: f64,
    /// Bars as `(is_bar, width in modules)` runs for 1D codes (empty for Data Matrix).
    pub runs: Vec<(bool, f64)>,
    pub paths: Vec<BezPath>,
}

impl BarcodeSpec {
    pub fn generate(&self) -> Result<BarcodeResult> {
        self.generate_with(&|s| s.to_string())
    }

    /// Generate after evaluating variable text in `data`.
    pub fn generate_with(&self, eval: &dyn Fn(&str) -> String) -> Result<BarcodeResult> {
        let data = eval(&self.data);
        if data.is_empty() {
            return Err(TextError::Barcode("nothing to encode".into()));
        }
        if !(self.module_mm.is_finite() && self.module_mm > 0.0) {
            return Err(TextError::Barcode("module size must be positive".into()));
        }
        let ratio = if self.wide_ratio.is_finite() { self.wide_ratio.clamp(2.0, 3.5) } else { 3.0 };
        if self.kind == BarcodeKind::DataMatrix {
            let dm = datamatrix::DataMatrix::encode_str(&data, datamatrix::SymbolList::default()).map_err(|e| TextError::Barcode(format!("{e:?}")))?;
            let bm = dm.bitmap();
            let (w, h) = (bm.width(), bm.height());
            let mut matrix = vec![vec![false; w]; h];
            for (x, y) in bm.pixels() {
                matrix[y][x] = true;
            }
            let paths = matrix_paths(&matrix, self.module_mm, true);
            let wm = w as f64 * self.module_mm;
            let hm = h as f64 * self.module_mm;
            return Ok(BarcodeResult {
                payload: data,
                width_mm: wm,
                height_mm: hm,
                total_width_mm: wm + 2.0 * self.quiet_zone.min(2) as f64 * self.module_mm,
                runs: Vec::new(),
                paths,
            });
        }
        let (payload, runs) = match self.kind {
            BarcodeKind::Code128 => code128(&data)?,
            BarcodeKind::Code39 => code39(&data, self.code39_check, ratio)?,
            BarcodeKind::Ean13 => ean13(&data, false)?,
            BarcodeKind::UpcA => ean13(&data, true)?,
            BarcodeKind::Itf => itf(&data, ratio)?,
            BarcodeKind::DataMatrix => unreachable!(),
        };
        let m = self.module_mm;
        let h = if self.height_mm.is_finite() && self.height_mm > 0.0 { self.height_mm } else { 10.0 };
        let mut x = 0.0;
        let mut bars: Vec<BezPath> = Vec::new();
        for (bar, w) in &runs {
            let wmm = w * m;
            if *bar {
                bars.push(Rect::new(x, 0.0, x + wmm, h).to_path(0.0));
            }
            x += wmm;
        }
        let paths = geomutil::weld(&bars, 0.01);
        Ok(BarcodeResult { payload, width_mm: x, height_mm: h, total_width_mm: x + 2.0 * self.quiet_zone as f64 * m, runs, paths })
    }
}

fn runs_from_widths(widths: &[f64], starts_with_bar: bool) -> Vec<(bool, f64)> {
    widths.iter().enumerate().map(|(i, w)| ((i % 2 == 0) == starts_with_bar, *w)).collect()
}

// ---------------------------------------------------------------- Code 128

/// Bar/space widths (modules) for symbol values 0..=106.
pub const CODE128_PATTERNS: [&str; 107] = [
    "212222", "222122", "222221", "121223", "121322", "131222", "122213", "122312", "132212", "221213", "221312", "231212", "112232", "122132", "122231",
    "113222", "123122", "123221", "223211", "221132", "221231", "213212", "223112", "312131", "311222", "321122", "321221", "312212", "322112", "322211",
    "212123", "212321", "232121", "111323", "131123", "131321", "112313", "132113", "132311", "211313", "231113", "231311", "112133", "112331", "132131",
    "113123", "113321", "133121", "313121", "211331", "231131", "213113", "213311", "213131", "311123", "311321", "331121", "312113", "312311", "332111",
    "314111", "221411", "431111", "111224", "111422", "121124", "121421", "141122", "141221", "112214", "112412", "122114", "122411", "142112", "142211",
    "241211", "221114", "413111", "241112", "134111", "111242", "121142", "121241", "114212", "124112", "124211", "411212", "421112", "421211", "212141",
    "214121", "412121", "111143", "111341", "131141", "114113", "114311", "411113", "411311", "113141", "114131", "311141", "411131", "211412", "211214",
    "211232", "2331112",
];

/// Code 128 symbol values for `data` (start code first, check symbol and stop last).
pub fn code128_values(data: &str) -> Result<Vec<u32>> {
    let bytes: Vec<u8> = data.chars().map(|c| if (' '..='~').contains(&c) { Ok(c as u8) } else { Err(TextError::Barcode(format!("Code 128: unsupported character {c:?}"))) }).collect::<Result<_>>()?;
    let n = bytes.len();
    let digit_run = |i: usize| bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    #[derive(PartialEq, Clone, Copy)]
    enum M {
        B,
        C,
    }
    let mut mode: Option<M> = None;
    let mut codes: Vec<u32> = Vec::new();
    let mut start = 104u32;
    let mut i = 0;
    while i < n {
        let d = digit_run(i);
        match mode {
            Some(M::C) => {
                if d >= 2 {
                    codes.push(((bytes[i] - b'0') * 10 + (bytes[i + 1] - b'0')) as u32);
                    i += 2;
                } else {
                    codes.push(100);
                    mode = Some(M::B);
                }
            }
            _ => {
                if d >= 4 {
                    if d % 2 == 1 {
                        if mode.is_none() {
                            start = 104;
                            mode = Some(M::B);
                        }
                        codes.push((bytes[i] - 32) as u32);
                        i += 1;
                    }
                    if mode.is_none() {
                        start = 105;
                    } else {
                        codes.push(99);
                    }
                    mode = Some(M::C);
                } else {
                    if mode.is_none() {
                        start = 104;
                        mode = Some(M::B);
                    }
                    codes.push((bytes[i] - 32) as u32);
                    i += 1;
                }
            }
        }
    }
    let sum: u32 = start + codes.iter().enumerate().map(|(k, c)| c * (k as u32 + 1)).sum::<u32>();
    let mut out = vec![start];
    out.extend(&codes);
    out.push(sum % 103);
    out.push(106);
    Ok(out)
}

fn code128(data: &str) -> Result<(String, Vec<(bool, f64)>)> {
    let vals = code128_values(data)?;
    let mut widths = Vec::new();
    for v in vals {
        widths.extend(CODE128_PATTERNS[v as usize].bytes().map(|b| (b - b'0') as f64));
    }
    Ok((data.to_string(), runs_from_widths(&widths, true)))
}

// ---------------------------------------------------------------- Code 39

const CODE39_CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-. $/+%*";
/// n = narrow, w = wide; nine elements starting with a bar.
pub const CODE39_PATTERNS: [&str; 44] = [
    "nnnwwnwnn", "wnnwnnnnw", "nnwwnnnnw", "wnwwnnnnn", "nnnwwnnnw", "wnnwwnnnn", "nnwwwnnnn", "nnnwnnwnw", "wnnwnnwnn", "nnwwnnwnn", "wnnnnwnnw", "nnwnnwnnw",
    "wnwnnwnnn", "nnnnwwnnw", "wnnnwwnnn", "nnwnwwnnn", "nnnnnwwnw", "wnnnnwwnn", "nnwnnwwnn", "nnnnwwwnn", "wnnnnnnww", "nnwnnnnww", "wnwnnnnwn", "nnnnwnnww",
    "wnnnwnnwn", "nnwnwnnwn", "nnnnnnwww", "wnnnnnwwn", "nnwnnnwwn", "nnnnwnwwn", "wwnnnnnnw", "nwwnnnnnw", "wwwnnnnnn", "nwnnwnnnw", "wwnnwnnnn", "nwwnwnnnn",
    "nwnnnnwnw", "wwnnnnwnn", "nwwnnnwnn", "nwnwnwnnn", "nwnwnnnwn", "nwnnnwnwn", "nnnwnwnwn", "nwnnwnwnn",
];

fn code39(data: &str, check: bool, ratio: f64) -> Result<(String, Vec<(bool, f64)>)> {
    let up = data.to_uppercase();
    let mut chars: Vec<usize> = Vec::new();
    for c in up.chars() {
        if c == '*' {
            return Err(TextError::Barcode("Code 39: '*' is reserved for start/stop".into()));
        }
        chars.push(CODE39_CHARS.find(c).ok_or_else(|| TextError::Barcode(format!("Code 39: unsupported character {c:?}")))?);
    }
    let mut payload = up.clone();
    if check {
        let sum: usize = chars.iter().sum();
        let ci = sum % 43;
        chars.push(ci);
        payload.push(CODE39_CHARS.chars().nth(ci).unwrap());
    }
    let star = CODE39_CHARS.len() - 1;
    let mut seq = vec![star];
    seq.extend(chars);
    seq.push(star);
    let mut runs = Vec::new();
    for (k, c) in seq.iter().enumerate() {
        if k > 0 {
            runs.push((false, 1.0));
        }
        let widths: Vec<f64> = CODE39_PATTERNS[*c].bytes().map(|b| if b == b'w' { ratio } else { 1.0 }).collect();
        runs.extend(runs_from_widths(&widths, true));
    }
    Ok((payload, runs))
}

// ---------------------------------------------------------------- EAN-13 / UPC-A

const EAN_L: [&str; 10] = ["0001101", "0011001", "0010011", "0111101", "0100011", "0110001", "0101111", "0111011", "0110111", "0001011"];
const EAN_PARITY: [&str; 10] = ["LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG", "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL"];

fn ean_check(d12: &[u8]) -> u8 {
    let s: u32 = d12.iter().enumerate().map(|(i, d)| *d as u32 * if i % 2 == 0 { 1 } else { 3 }).sum();
    ((10 - s % 10) % 10) as u8
}

fn ean13(data: &str, upc: bool) -> Result<(String, Vec<(bool, f64)>)> {
    let mut d: Vec<u8> = data.chars().map(|c| c.to_digit(10).map(|v| v as u8).ok_or_else(|| TextError::Barcode("EAN/UPC: digits only".into()))).collect::<Result<_>>()?;
    if upc {
        // UPC-A is EAN-13 with a leading zero.
        if d.len() == 11 || d.len() == 12 {
            d.insert(0, 0);
        } else {
            return Err(TextError::Barcode("UPC-A needs 11 or 12 digits".into()));
        }
    }
    match d.len() {
        12 => {
            let c = ean_check(&d);
            d.push(c);
        }
        13 => {
            if ean_check(&d[..12]) != d[12] {
                return Err(TextError::Barcode("EAN/UPC: wrong check digit".into()));
            }
        }
        _ => return Err(TextError::Barcode("EAN-13 needs 12 or 13 digits".into())),
    }
    let mut bits = String::from("101");
    let parity = EAN_PARITY[d[0] as usize].as_bytes();
    for (i, digit) in d[1..7].iter().enumerate() {
        let l = EAN_L[*digit as usize];
        if parity[i] == b'L' {
            bits.push_str(l);
        } else {
            // G code: reverse of the R code, R is the complement of L.
            let r: String = l.chars().map(|c| if c == '0' { '1' } else { '0' }).collect();
            bits.extend(r.chars().rev());
        }
    }
    bits.push_str("01010");
    for digit in &d[7..13] {
        bits.extend(EAN_L[*digit as usize].chars().map(|c| if c == '0' { '1' } else { '0' }));
    }
    bits.push_str("101");
    let payload: String = d.iter().map(|x| char::from(b'0' + x)).collect();
    let payload = if upc { payload[1..].to_string() } else { payload };
    Ok((payload, bits_to_runs(&bits)))
}

/// Run-length encode a `0`/`1` string into `(is_bar, modules)`.
pub fn bits_to_runs(bits: &str) -> Vec<(bool, f64)> {
    let mut runs: Vec<(bool, f64)> = Vec::new();
    for c in bits.chars() {
        let bar = c == '1';
        match runs.last_mut() {
            Some((b, w)) if *b == bar => *w += 1.0,
            _ => runs.push((bar, 1.0)),
        }
    }
    runs
}

// ---------------------------------------------------------------- ITF

const ITF: [&str; 10] = ["nnwwn", "wnnnw", "nwnnw", "wwnnn", "nnwnw", "wnwnn", "nwwnn", "nnnww", "wnnwn", "nwnwn"];

fn itf(data: &str, ratio: f64) -> Result<(String, Vec<(bool, f64)>)> {
    let mut d: Vec<usize> = data.chars().map(|c| c.to_digit(10).map(|v| v as usize).ok_or_else(|| TextError::Barcode("ITF: digits only".into()))).collect::<Result<_>>()?;
    if d.len() % 2 == 1 {
        d.insert(0, 0);
    }
    let w = |c: u8| if c == b'w' { ratio } else { 1.0 };
    let mut widths: Vec<f64> = vec![1.0, 1.0, 1.0, 1.0];
    for pair in d.chunks(2) {
        let (a, b) = (ITF[pair[0]].as_bytes(), ITF[pair[1]].as_bytes());
        for k in 0..5 {
            widths.push(w(a[k]));
            widths.push(w(b[k]));
        }
    }
    widths.extend([ratio, 1.0, 1.0]);
    Ok((d.iter().map(|x| char::from(b'0' + *x as u8)).collect(), runs_from_widths(&widths, true)))
}
