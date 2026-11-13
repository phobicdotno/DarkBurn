//! PLT / HPGL import and export.
//!
//! Supported instructions: `IN`, `SP`, `PU`, `PD`, `PA`, `PR`, `CI`, `AA`, `AR`, `EA`, `ER`, `PG`.
//! Labels (`LB`), encoded polylines (`PE`), scaling (`SC`) and device escapes are skipped
//! with a warning. One plotter unit is 0.025 mm by default (40 units per mm).

use std::collections::BTreeMap;

use kurbo::{BezPath, Point, Vec2};

use crate::geom;
use crate::model::*;
use crate::{ImportError, ItemKind};

/// HPGL import options.
#[derive(Clone, Debug)]
pub struct HpglOptions {
    /// Plotter units per millimetre (precision). The HPGL standard is 40.
    pub units_per_mm: f64,
}

impl Default for HpglOptions {
    fn default() -> Self {
        HpglOptions { units_per_mm: 40.0 }
    }
}

/// Colour shown for a pen number (pen 1 black, 2 red, 3 green, 4 blue, ...).
pub fn pen_colour(pen: u32) -> Rgb {
    const PALETTE: [Rgb; 8] =
        [Rgb(0, 0, 0), Rgb(255, 0, 0), Rgb(0, 160, 0), Rgb(0, 0, 255), Rgb(0, 170, 170), Rgb(200, 0, 200), Rgb(200, 160, 0), Rgb(255, 128, 0)];
    PALETTE[(pen.max(1) as usize - 1) % PALETTE.len()]
}

struct State {
    pos: Point,
    down: bool,
    relative: bool,
    pen: u32,
    current: Option<BezPath>,
    out: Imported,
    budget: usize,
}

impl State {
    fn flush(&mut self) {
        if let Some(p) = self.current.take()
            && p.elements().len() > 1
            && self.budget > 0
        {
            self.budget -= 1;
            let mut p = p;
            if let (Some(kurbo::PathEl::MoveTo(s)), Some(e)) = (p.elements().first().copied(), p.elements().last().and_then(|e| e.end_point()))
                && (s - e).hypot() < 1e-9
                && p.elements().len() > 2
            {
                p.close_path();
            }
            let item = Item::path(p, Some(pen_colour(self.pen)), None).with_layer(format!("Pen {}", self.pen));
            self.out.items.push(item);
        }
    }

    fn move_to(&mut self, p: Point) {
        self.flush();
        self.pos = p;
    }

    fn line_to(&mut self, p: Point) {
        let pos = self.pos;
        let cur = self.current.get_or_insert_with(|| {
            let mut b = BezPath::new();
            b.move_to(pos);
            b
        });
        cur.line_to(p);
        self.pos = p;
    }

    fn ensure_started(&mut self) {
        if self.current.is_none() {
            let mut b = BezPath::new();
            b.move_to(self.pos);
            self.current = Some(b);
        }
    }

    fn go(&mut self, p: Point) {
        if self.down { self.line_to(p) } else { self.move_to(p) }
    }
}

fn parse_params(s: &str) -> Vec<f64> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .collect()
}

/// The index just past the device escape that starts at `chars[i]` (an ESC), so that only the
/// escape itself is skipped, never the instruction after it.
///
/// - HP-GL device control, `ESC.` and one letter: plotter on/off (`(`, `Y`, `)`, `Z`) and the
///   output and abort requests (`A`, `B`, `E`, `J`, `K`, `L`, `O`, `R`) end there; the others
///   take numbers separated by `;` and end with `:` (`ESC.@1024;0:`, `ESC.I81;;17:`).
/// - PCL with parameters, `ESC` then a character from `!` to `/`, an optional group letter
///   from `` ` `` to `~`, then values, each followed by a lowercase letter (more follow) or by an
///   uppercase letter from `@` to `^` (the end): `ESC%0B`, `ESC%-12345X`, `ESC&l1o0E`.
/// - Two-character PCL escapes, `ESC` and one character from `0` to `~`: `ESCE` (reset).
fn skip_escape(chars: &[char], mut i: usize) -> usize {
    let at = |k: usize| chars.get(k).copied().unwrap_or('\0');
    i += 1;
    match at(i) {
        '.' => {
            i += 1;
            let kind = at(i);
            if i < chars.len() {
                i += 1;
            }
            if !matches!(kind, '(' | 'Y' | ')' | 'Z' | 'A' | 'B' | 'E' | 'J' | 'K' | 'L' | 'O' | 'R') {
                while at(i).is_ascii_digit() || matches!(at(i), ';' | '+' | '-' | '.' | ' ') {
                    i += 1;
                }
                if at(i) == ':' {
                    i += 1;
                }
            }
            i
        }
        '!'..='/' => {
            i += 1;
            if matches!(at(i), '`'..='~') {
                i += 1;
            }
            loop {
                while at(i).is_ascii_digit() || matches!(at(i), '+' | '-' | '.') {
                    i += 1;
                }
                match at(i) {
                    '`'..='~' => i += 1,
                    '@'..='^' => return i + 1,
                    _ => return i,
                }
            }
        }
        '0'..='~' => i + 1,
        _ => i,
    }
}

/// Imports HPGL text.
pub fn import(bytes: &[u8], opts: &HpglOptions) -> Result<Imported, ImportError> {
    import_with_budget(bytes, opts, 2_000_000)
}

/// [`import`] with `budget` the most items (lines, circles, arcs, rectangles) it keeps.
fn import_with_budget(bytes: &[u8], opts: &HpglOptions, budget: usize) -> Result<Imported, ImportError> {
    let upm = if opts.units_per_mm.is_finite() && opts.units_per_mm > 0.0 { opts.units_per_mm } else { 40.0 };
    let text = String::from_utf8_lossy(bytes);
    let chars: Vec<char> = text.chars().collect();
    let mut st = State { pos: Point::ZERO, down: false, relative: false, pen: 1, current: None, out: Imported::default(), budget };
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut commands = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{1b}' {
            i = skip_escape(&chars, i);
            continue;
        }
        if !c.is_ascii_alphabetic() || i + 1 >= chars.len() || !chars[i + 1].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let cmd: String = [c.to_ascii_uppercase(), chars[i + 1].to_ascii_uppercase()].iter().collect();
        i += 2;
        if cmd == "LB" {
            while i < chars.len() && chars[i] != '\u{3}' {
                i += 1;
            }
            i += 1;
            *skipped.entry("LB (labels)".into()).or_default() += 1;
            continue;
        }
        let start = i;
        // An `e` is part of the parameters only as an exponent (a digit or sign follows);
        // otherwise it starts the next instruction (`PD400,0EA800,800`).
        let exponent = |k: usize| matches!(chars[k], 'e' | 'E') && chars.get(k + 1).is_some_and(|n| n.is_ascii_digit() || matches!(n, '+' | '-'));
        while i < chars.len() && chars[i] != ';' && !(chars[i].is_ascii_alphabetic() && !exponent(i)) {
            i += 1;
        }
        let params_str: String = chars[start..i].iter().collect();
        if i < chars.len() && chars[i] == ';' {
            i += 1;
        }
        let params = parse_params(&params_str);
        if matches!(
            cmd.as_str(),
            "IN" | "DF" | "SP" | "PU" | "PD" | "PA" | "PR" | "CI" | "AA" | "AR" | "EA" | "ER" | "PG" | "PE" | "SC" | "IP" | "LT" | "VS" | "PW" | "WU"
        ) {
            commands += 1;
        }
        let to_pt = |x: f64, y: f64| Point::new(x / upm, y / upm);
        match cmd.as_str() {
            "IN" | "DF" => {
                st.flush();
                st.pos = Point::ZERO;
                st.down = false;
                st.relative = false;
            }
            "SP" => {
                st.flush();
                st.pen = params.first().map(|v| v.max(0.0) as u32).unwrap_or(0).max(1);
            }
            "PU" | "PD" | "PA" | "PR" => {
                match cmd.as_str() {
                    "PU" => {
                        st.down = false;
                        st.flush();
                    }
                    "PD" => st.down = true,
                    "PA" => st.relative = false,
                    _ => st.relative = true,
                }
                for pair in params.as_chunks::<2>().0 {
                    let d = to_pt(pair[0], pair[1]);
                    let target = if st.relative { st.pos + d.to_vec2() } else { d };
                    st.go(target);
                }
            }
            "CI" => {
                let Some(&r) = params.first() else { continue };
                let r = r.abs() / upm;
                let center = st.pos;
                st.flush();
                let p = geom::circle_path(center, r);
                if !p.is_empty() && st.budget > 0 {
                    st.budget -= 1;
                    st.out.items.push(Item::path(p, Some(pen_colour(st.pen)), None).with_layer(format!("Pen {}", st.pen)));
                }
                st.pos = center;
            }
            "AA" | "AR" => {
                if params.len() < 3 {
                    continue;
                }
                let c = to_pt(params[0], params[1]);
                let center = if cmd == "AR" { st.pos + c.to_vec2() } else { c };
                let sweep = params[2].to_radians();
                let r = (st.pos - center).hypot();
                let start = (st.pos - center).atan2();
                let end = center + Vec2::from_angle(start + sweep) * r;
                if st.down && r > 0.0 {
                    st.ensure_started();
                    if let Some(cur) = st.current.as_mut() {
                        geom::append_arc(cur, center, r, r, 0.0, start, sweep);
                    }
                    st.pos = end;
                } else {
                    st.move_to(end);
                }
            }
            "EA" | "ER" => {
                if params.len() < 2 {
                    continue;
                }
                let d = to_pt(params[0], params[1]);
                let corner = if cmd == "ER" { st.pos + d.to_vec2() } else { d };
                let o = st.pos;
                st.flush();
                if st.budget == 0 {
                    continue;
                }
                st.budget -= 1;
                let mut p = BezPath::new();
                p.move_to(o);
                p.line_to((corner.x, o.y));
                p.line_to(corner);
                p.line_to((o.x, corner.y));
                p.close_path();
                st.out.items.push(Item::path(p, Some(pen_colour(st.pen)), None).with_layer(format!("Pen {}", st.pen)));
            }
            "PG" => st.flush(),
            "PE" => *skipped.entry("PE (encoded polylines)".into()).or_default() += 1,
            "SC" => *skipped.entry("SC (scaling)".into()).or_default() += 1,
            _ => {}
        }
    }
    st.flush();
    let mut out = st.out;
    if commands == 0 {
        return Err(ImportError::Parse("no HPGL instructions found".into()));
    }
    for (k, n) in skipped {
        out.warn(format!("{n} HPGL {k} instructions were ignored"));
    }
    Ok(crate::finish(out))
}

/// HPGL export options.
#[derive(Clone, Debug)]
pub struct HpglExportOptions {
    pub units_per_mm: f64,
    /// Curve flattening tolerance in mm.
    pub tolerance: f64,
}

impl Default for HpglExportOptions {
    fn default() -> Self {
        HpglExportOptions { units_per_mm: 40.0, tolerance: 0.025 }
    }
}

/// Writes items as HPGL. Each distinct colour gets its own pen number in order of first use.
/// Text is written from its outline when it has one; images are left out.
pub fn export(items: &[Item], opts: &HpglExportOptions) -> String {
    let upm = if opts.units_per_mm > 0.0 { opts.units_per_mm } else { 40.0 };
    let mut pens: Vec<Rgb> = Vec::new();
    let mut by_pen: BTreeMap<usize, Vec<&BezPath>> = BTreeMap::new();
    for item in items.iter().filter(|i| !i.hidden) {
        let (path, colour) = match &item.kind {
            ItemKind::Path(p) => (&p.path, p.stroke.or(p.fill).unwrap_or(Rgb::BLACK)),
            ItemKind::Text(t) => match &t.outline {
                Some(o) => (o, t.colour),
                None => continue,
            },
            ItemKind::Image(_) => continue,
        };
        let pen = match pens.iter().position(|c| *c == colour) {
            Some(i) => i,
            None => {
                pens.push(colour);
                pens.len() - 1
            }
        };
        by_pen.entry(pen).or_default().push(path);
    }
    let mut s = String::from("IN;\n");
    let q = |v: f64| (v * upm).round() as i64;
    for (pen, paths) in by_pen {
        s.push_str(&format!("SP{};\n", pen + 1));
        for path in paths {
            for (pts, closed) in geom::flatten_polylines(path, opts.tolerance) {
                let mut pts: Vec<(i64, i64)> = pts.iter().map(|p| (q(p.x), q(p.y))).collect();
                pts.dedup();
                if closed && pts.first() != pts.last() {
                    pts.push(pts[0]);
                }
                if pts.len() < 2 {
                    continue;
                }
                s.push_str(&format!("PU{},{};\nPD", pts[0].0, pts[0].1));
                let body: Vec<String> = pts[1..].iter().map(|(x, y)| format!("{x},{y}")).collect();
                s.push_str(&body.join(","));
                s.push_str(";\n");
            }
        }
    }
    s.push_str("PU;SP0;\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(src: &str, budget: usize) -> usize {
        import_with_budget(src.as_bytes(), &HpglOptions::default(), budget).unwrap().items.len()
    }

    #[test]
    fn edge_rectangles_count_against_the_item_budget() {
        // Lines spend the budget; rectangles ignored it, so a file of millions of EA/ER
        // instructions could still pile up without limit.
        assert_eq!(paths("IN;PA0,0;EA400,400;EA800,800;ER40,40;", 1), 1);
        // Rectangles and lines share one counter.
        assert_eq!(paths("IN;PA0,0;EA400,400;PD800,800;PU;", 1), 1);
        assert_eq!(paths("IN;PD400,400;PU;EA800,800;", 1), 1);
        assert_eq!(paths("IN;PD400,400;PU;EA800,800;", 2), 2);
    }
}
