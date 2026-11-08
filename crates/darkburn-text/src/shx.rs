//! AutoCAD SHX single-line (stroke) fonts.
//!
//! Clean-room reader written from the publicly documented shape-file format (header, shape table,
//! the vector/special byte codes). Supported: `AutoCAD-86 shapes 1.0` and `AutoCAD-86 unifont 1.0`.
//! `bigfont` files are rejected. Output is open paths in mm, Y up.
//!
//! Notes on the format (as implemented):
//! * special codes 0 end, 1 pen down, 2 pen up, 3/4 divide/multiply scale, 5/6 push/pop position,
//!   7 sub-shape, 8 XY displacement, 9 XY list, 10 octant arc, 11 fractional arc, 12 bulge arc,
//!   13 bulge arc list, 14 vertical-text-only prefix (skipped for horizontal text);
//! * codes 16 and up are vectors: high nibble length, low nibble one of 16 directions;
//! * the pen starts down; a character advances by the x position its shape ends at.
//!
//! The fractional-arc end offset is the least certain detail; see [`ShxFont`] tests.

use crate::error::{Result, TextError};
use crate::geomutil;
use crate::layout::{GlyphOutline, HAlign, LineMetrics, TextLayout, TextMetrics, TextSpec, VAlign};
use kurbo::{Affine, Arc, BezPath, Point, Rect, Vec2};
use std::collections::HashMap;

const DIRS: [(f64, f64); 16] = [
    (1.0, 0.0),
    (1.0, 0.5),
    (1.0, 1.0),
    (0.5, 1.0),
    (0.0, 1.0),
    (-0.5, 1.0),
    (-1.0, 1.0),
    (-1.0, 0.5),
    (-1.0, 0.0),
    (-1.0, -0.5),
    (-1.0, -1.0),
    (-0.5, -1.0),
    (0.0, -1.0),
    (0.5, -1.0),
    (1.0, -1.0),
    (1.0, -0.5),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Shapes,
    Unifont,
}

/// A parsed SHX font.
#[derive(Clone, Debug)]
pub struct ShxFont {
    pub name: String,
    /// Height of capitals above the baseline, in shape units.
    pub above: f64,
    /// Depth of descenders below the baseline, in shape units.
    pub below: f64,
    pub modes: u8,
    kind: Kind,
    shapes: HashMap<u32, Vec<u8>>,
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8> {
        let v = *self.d.get(self.p).ok_or_else(|| TextError::Shx("unexpected end of file".into()))?;
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(self.u8()? as u16 | (self.u8()? as u16) << 8)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(self.u16()? as u32 | (self.u16()? as u32) << 16)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|e| *e <= self.d.len()).ok_or_else(|| TextError::Shx("shape runs past end of file".into()))?;
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
}

/// Split a definition into its null-terminated name and the command bytes.
fn split_name(def: &[u8]) -> (String, &[u8]) {
    match def.iter().position(|b| *b == 0) {
        Some(i) => (String::from_utf8_lossy(&def[..i]).into_owned(), &def[i + 1..]),
        None => (String::new(), &[]),
    }
}

impl ShxFont {
    /// Parse the bytes of a `.shx` file.
    pub fn parse(data: &[u8]) -> Result<ShxFont> {
        let hdr_end = data.iter().take(80).position(|b| *b == 0x1A).ok_or_else(|| TextError::Shx("missing header terminator".into()))?;
        let header = String::from_utf8_lossy(&data[..hdr_end]).to_lowercase();
        let kind = if header.starts_with("autocad-86 shapes 1.0") {
            Kind::Shapes
        } else if header.starts_with("autocad-86 unifont 1.0") {
            Kind::Unifont
        } else if header.contains("bigfont") {
            return Err(TextError::Shx("bigfont SHX files are not supported".into()));
        } else {
            return Err(TextError::Shx(format!("unknown header {:?}", header.trim())));
        };
        let mut r = Rd { d: data, p: hdr_end + 1 };
        let mut shapes: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut font_def: Option<Vec<u8>> = None;
        match kind {
            Kind::Shapes => {
                let _first = r.u16()?;
                let _last = r.u16()?;
                let count = r.u16()? as usize;
                let mut table = Vec::with_capacity(count);
                for _ in 0..count {
                    table.push((r.u16()? as u32, r.u16()? as usize));
                }
                for (num, len) in table {
                    let def = r.take(len)?.to_vec();
                    if num == 0 {
                        font_def = Some(def);
                    } else {
                        shapes.insert(num, def);
                    }
                }
            }
            Kind::Unifont => {
                let count = r.u32()? as usize;
                let def_len = r.u16()? as usize;
                font_def = Some(r.take(def_len)?.to_vec());
                for _ in 0..count {
                    let num = r.u16()? as u32;
                    let len = r.u16()? as usize;
                    let def = r.take(len)?.to_vec();
                    shapes.insert(num, def);
                }
            }
        }
        let fd = font_def.ok_or_else(|| TextError::Shx("font definition (shape 0) missing".into()))?;
        let (name, rest) = split_name(&fd);
        if rest.len() < 2 {
            return Err(TextError::Shx("font definition too short".into()));
        }
        let (above, below, modes) = (rest[0] as f64, rest[1] as f64, rest.get(2).copied().unwrap_or(0));
        if above <= 0.0 {
            return Err(TextError::Shx("font has zero height".into()));
        }
        // Store only the command bytes of each shape.
        let shapes = shapes.into_iter().map(|(k, v)| (k, split_name(&v).1.to_vec())).collect();
        Ok(ShxFont { name, above, below, modes, kind, shapes })
    }

    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<ShxFont> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn has_glyph(&self, ch: char) -> bool {
        self.shapes.contains_key(&(ch as u32))
    }

    pub fn glyph_count(&self) -> usize {
        self.shapes.len()
    }

    /// Open paths and advance of one character in shape units.
    pub fn glyph_units(&self, ch: char) -> Option<(BezPath, f64)> {
        let def = self.shapes.get(&(ch as u32))?;
        let mut it = Interp::new(self);
        it.run(def, 0);
        Some((it.path, it.pos.x))
    }

    /// Lay out text. Uses `spec.text`, `size`, `size_mode` (cap height = the font's `above`),
    /// `upper_case`, `letter_spacing`, `line_spacing`, `italic`, `h_align` and `v_align`.
    pub fn layout(&self, spec: &TextSpec) -> TextLayout {
        use crate::layout::SizeMode;
        let size = if spec.size.is_finite() && spec.size > 0.0 { spec.size } else { 10.0 };
        let s = match spec.size_mode {
            SizeMode::CapHeight => size / self.above,
            SizeMode::Em => size / (self.above + self.below).max(1.0),
        };
        let text = if spec.upper_case { spec.text.to_uppercase() } else { spec.text.clone() };
        let metrics = TextMetrics {
            em_mm: (self.above + self.below) * s,
            cap_height_mm: self.above * s,
            ascent_mm: self.above * s,
            descent_mm: -self.below * s,
            line_height_mm: (self.above + self.below) * s * 1.3 + spec.line_spacing,
        };
        let shear = if spec.italic { 0.2126 } else { 0.0 };
        let mut glyphs = Vec::new();
        let mut lines = Vec::new();
        for (li, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let baseline_y = -(li as f64) * metrics.line_height_mm;
            let first = glyphs.len();
            let mut pen = 0.0;
            for ch in line.chars() {
                let (path, adv_u) = self.glyph_units(ch).unwrap_or((BezPath::new(), self.above * 0.6));
                let adv = adv_u * s;
                let xf = Affine::translate((pen, baseline_y)) * Affine::scale(s) * Affine::new([1.0, 0.0, shear, 1.0, 0.0, 0.0]);
                glyphs.push(GlyphOutline {
                    text: ch.to_string(),
                    line: li,
                    path: xf * path,
                    origin_x: pen,
                    baseline_y,
                    advance: adv + spec.letter_spacing,
                    rtl: false,
                    missing: !self.has_glyph(ch) && !ch.is_whitespace(),
                });
                pen += adv + spec.letter_spacing;
            }
            let width = pen;
            let x_start = match spec.h_align {
                HAlign::Left => 0.0,
                HAlign::Center => -width / 2.0,
                HAlign::Right => -width,
            };
            for g in &mut glyphs[first..] {
                g.path.apply_affine(Affine::translate((x_start, 0.0)));
                g.origin_x += x_start;
            }
            lines.push(LineMetrics { baseline_y, width, x_start });
        }
        let n = lines.len().max(1) as f64;
        let top = metrics.cap_height_mm;
        let bottom = -(n - 1.0) * metrics.line_height_mm;
        let dy = match spec.v_align {
            VAlign::Top => -top,
            VAlign::Bottom => -bottom,
            VAlign::Middle => -(top + bottom) / 2.0,
        };
        if dy != 0.0 {
            for g in &mut glyphs {
                g.path.apply_affine(Affine::translate((0.0, dy)));
                g.baseline_y += dy;
            }
            for l in &mut lines {
                l.baseline_y += dy;
            }
        }
        let bounds = geomutil::union_bbox(glyphs.iter().map(|g| &g.path)).unwrap_or(Rect::ZERO);
        TextLayout { glyphs, lines, metrics, bounds, family_used: self.name.clone() }
    }
}

struct Interp<'a> {
    font: &'a ShxFont,
    path: BezPath,
    pos: Point,
    pen_down: bool,
    active: bool,
    scale: f64,
    stack: Vec<Point>,
    steps: usize,
}

impl<'a> Interp<'a> {
    fn new(font: &'a ShxFont) -> Self {
        Interp { font, path: BezPath::new(), pos: Point::ZERO, pen_down: true, active: false, scale: 1.0, stack: Vec::new(), steps: 0 }
    }

    fn line_to(&mut self, p: Point) {
        if self.pen_down {
            if !self.active {
                self.path.move_to(self.pos);
                self.active = true;
            }
            self.path.line_to(p);
        } else {
            self.active = false;
        }
        self.pos = p;
    }

    fn arc(&mut self, center: Point, r: f64, a0: f64, sweep: f64) {
        let end = center + Vec2::new(r * (a0 + sweep).cos(), r * (a0 + sweep).sin());
        if self.pen_down && r > 0.0 {
            if !self.active {
                self.path.move_to(self.pos);
                self.active = true;
            }
            let arc = Arc::new(center, (r, r), a0, sweep, 0.0);
            self.path.extend(arc.append_iter((r * 1e-3).max(1e-6)));
        } else {
            self.active = false;
        }
        self.pos = end;
    }

    fn bulge(&mut self, dx: f64, dy: f64, b: i8) {
        let p1 = self.pos + Vec2::new(dx, dy) * self.scale;
        if b == 0 {
            self.line_to(p1);
            return;
        }
        let bulge = b as f64 / 127.0;
        let theta = 4.0 * bulge.atan();
        let chord = p1 - self.pos;
        let c = chord.hypot();
        if c < 1e-12 {
            return;
        }
        let dir = chord / c;
        let left = Vec2::new(-dir.y, dir.x);
        let mid = self.pos.midpoint(p1);
        let center = mid + left * ((c / 2.0) / (theta / 2.0).tan());
        let r = (self.pos - center).hypot();
        let a0 = (self.pos.y - center.y).atan2(self.pos.x - center.x);
        self.arc(center, r, a0, theta);
        self.pos = p1;
    }

    fn run(&mut self, code: &[u8], depth: usize) {
        if depth > 8 {
            return;
        }
        let mut i = 0usize;
        let mut skip_next = false;
        let rd = |i: &mut usize| -> Option<u8> {
            let v = code.get(*i).copied();
            *i += 1;
            v
        };
        while i < code.len() {
            self.steps += 1;
            if self.steps > 100_000 {
                return;
            }
            let Some(op) = rd(&mut i) else { return };
            // The command after code 14 is for vertical text only: run it on a scratch state.
            let saved =
                if skip_next { Some((self.pos, self.pen_down, self.active, self.scale, self.stack.clone(), self.path.clone())) } else { None };
            skip_next = false;
            match op {
                0 => return,
                1 => self.pen_down = true,
                2 => {
                    self.pen_down = false;
                    self.active = false;
                }
                3 => {
                    let Some(v) = rd(&mut i) else { return };
                    if v != 0 {
                        self.scale /= v as f64;
                    }
                }
                4 => {
                    let Some(v) = rd(&mut i) else { return };
                    self.scale *= v as f64;
                }
                5 => {
                    if self.stack.len() < 16 {
                        self.stack.push(self.pos);
                    }
                }
                6 => {
                    if let Some(p) = self.stack.pop() {
                        self.pos = p;
                        self.active = false;
                    }
                }
                7 => {
                    let num = match self.font.kind {
                        Kind::Shapes => rd(&mut i).map(|v| v as u32),
                        Kind::Unifont => match (rd(&mut i), rd(&mut i)) {
                            (Some(a), Some(b)) => Some(a as u32 | (b as u32) << 8),
                            _ => None,
                        },
                    };
                    let Some(num) = num else { return };
                    if let Some(sub) = self.font.shapes.get(&num) {
                        let keep = self.scale;
                        self.run(sub, depth + 1);
                        self.scale = keep;
                    }
                }
                8 => {
                    let (Some(x), Some(y)) = (rd(&mut i), rd(&mut i)) else { return };
                    let p = self.pos + Vec2::new(x as i8 as f64, y as i8 as f64) * self.scale;
                    self.line_to(p);
                }
                9 => loop {
                    let (Some(x), Some(y)) = (rd(&mut i), rd(&mut i)) else { return };
                    if x == 0 && y == 0 {
                        break;
                    }
                    let p = self.pos + Vec2::new(x as i8 as f64, y as i8 as f64) * self.scale;
                    self.line_to(p);
                },
                10 => {
                    let (Some(r), Some(sc)) = (rd(&mut i), rd(&mut i)) else { return };
                    let cw = sc & 0x80 != 0;
                    let oct = ((sc >> 4) & 7) as f64;
                    let mut n = (sc & 0x0F) as f64;
                    if n == 0.0 {
                        n = 8.0;
                    }
                    let a0 = oct * std::f64::consts::FRAC_PI_4;
                    let sweep = n * std::f64::consts::FRAC_PI_4 * if cw { -1.0 } else { 1.0 };
                    let r = r as f64 * self.scale;
                    let center = self.pos - Vec2::new(r * a0.cos(), r * a0.sin());
                    self.arc(center, r, a0, sweep);
                }
                11 => {
                    let (Some(so), Some(eo), Some(hi), Some(lo), Some(sc)) = (rd(&mut i), rd(&mut i), rd(&mut i), rd(&mut i), rd(&mut i)) else {
                        return;
                    };
                    let cw = sc & 0x80 != 0;
                    let oct = ((sc >> 4) & 7) as f64;
                    let n = (sc & 0x0F) as f64;
                    let o = std::f64::consts::FRAC_PI_4;
                    let a0 = oct * o + so as f64 * o / 256.0;
                    let end_part = if eo == 0 { o } else { eo as f64 * o / 256.0 };
                    let a1 = (oct + (n - 1.0).max(0.0)) * o + end_part;
                    let sweep = if cw {
                        // Clockwise: mirror the construction.
                        -(((a0 - a1).rem_euclid(2.0 * std::f64::consts::PI)).max(1e-9))
                    } else {
                        (a1 - a0).rem_euclid(2.0 * std::f64::consts::PI).max(1e-9)
                    };
                    let r = (hi as f64 * 256.0 + lo as f64) * self.scale;
                    let center = self.pos - Vec2::new(r * a0.cos(), r * a0.sin());
                    self.arc(center, r, a0, sweep);
                }
                12 => {
                    let (Some(x), Some(y), Some(b)) = (rd(&mut i), rd(&mut i), rd(&mut i)) else { return };
                    self.bulge(x as i8 as f64, y as i8 as f64, b as i8);
                }
                13 => loop {
                    let (Some(x), Some(y)) = (rd(&mut i), rd(&mut i)) else { return };
                    if x == 0 && y == 0 {
                        break;
                    }
                    let Some(b) = rd(&mut i) else { return };
                    self.bulge(x as i8 as f64, y as i8 as f64, b as i8);
                },
                14 => skip_next = true,
                15 => {}
                _ => {
                    let len = (op >> 4) as f64;
                    let (dx, dy) = DIRS[(op & 15) as usize];
                    let p = self.pos + Vec2::new(dx, dy) * (len * self.scale);
                    self.line_to(p);
                }
            }
            if let Some((pos, pd, act, sc, st, path)) = saved {
                self.pos = pos;
                self.pen_down = pd;
                self.active = act;
                self.scale = sc;
                self.stack = st;
                self.path = path;
            }
        }
    }
}

/// Build a tiny `shapes 1.0` SHX file. `shapes` are `(number, name, command bytes)`; the command
/// bytes must include their terminating `0`. Useful for tests and as a minimal-font generator.
pub fn write_shapes_1_0(font_name: &str, above: u8, below: u8, shapes: &[(u16, &str, Vec<u8>)]) -> Vec<u8> {
    let mut defs: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut d0 = font_name.as_bytes().to_vec();
    d0.push(0);
    d0.extend([above, below, 0, 0]);
    defs.push((0, d0));
    for (n, name, code) in shapes {
        let mut d = name.as_bytes().to_vec();
        d.push(0);
        d.extend(code);
        defs.push((*n, d));
    }
    let mut out = b"AutoCAD-86 shapes 1.0\r\n\x1a".to_vec();
    let last = defs.iter().map(|d| d.0).max().unwrap_or(0);
    out.extend(0u16.to_le_bytes());
    out.extend(last.to_le_bytes());
    out.extend((defs.len() as u16).to_le_bytes());
    for (n, d) in &defs {
        out.extend(n.to_le_bytes());
        out.extend((d.len() as u16).to_le_bytes());
    }
    for (_, d) in &defs {
        out.extend(d);
    }
    out
}
