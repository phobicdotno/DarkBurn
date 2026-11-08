//! G-code output for GRBL 1.1, grblHAL, FluidNC, Marlin, Smoothieware and LinuxCNC.
//!
//! Written from the controllers' public documentation of their G-code dialects.

use std::collections::VecDeque;

use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::ir::{CoordSpace, IrLayer, Op, Toolpath};

/// G-code dialect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Flavor {
    Grbl,
    GrblHal,
    FluidNc,
    Marlin,
    Smoothieware,
    LinuxCnc,
}

impl Flavor {
    fn is_grbl(self) -> bool {
        matches!(self, Flavor::Grbl | Flavor::GrblHal | Flavor::FluidNc)
    }

    fn name(self) -> &'static str {
        match self {
            Flavor::Grbl => "GRBL",
            Flavor::GrblHal => "grblHAL",
            Flavor::FluidNc => "FluidNC",
            Flavor::Marlin => "Marlin",
            Flavor::Smoothieware => "Smoothieware",
            Flavor::LinuxCnc => "LinuxCNC",
        }
    }
}

/// How Marlin sets laser power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarlinPower {
    /// `M3 S..` / `M5` around moves.
    M3,
    /// Fan PWM: `M106 S..` / `M107`.
    M106,
    /// Inline power: `S` on every `G1` (needs `LASER_POWER_INLINE`).
    Inline,
}

/// Air assist command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AirCommand {
    /// Flood coolant `M8` (off `M9`).
    M8,
    /// Mist coolant `M7` (off `M9`).
    M7,
}

/// G-code writer settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GcodeSettings {
    pub flavor: Flavor,
    /// S value at 100 % power (GRBL `$30`, often 1000).
    pub s_max: f64,
    pub decimals: usize,
    /// Fewer decimals for fill and image layers.
    pub raster_decimals: usize,
    /// Write S on every G1 even when it did not change.
    pub s_every_g1: bool,
    /// Use G0 for overscan moves instead of G1 at power 0.
    pub overscan_as_g0: bool,
    pub air: AirCommand,
    pub marlin_power: MarlinPower,
    /// Marlin tool index (`T<n>`) selected at the start; laser 2 selects `T<n+1>`.
    pub tool_index: Option<u8>,
    /// LinuxCNC analog output used for power (`M67 E<n>`).
    pub analog_output: u8,
    /// Lines written before the job. `{name}` is replaced from `variables` and built-ins
    /// (`flavor`, `s_max`).
    pub start_script: String,
    pub end_script: String,
    pub variables: Vec<(String, String)>,
    pub comments: bool,
}

impl GcodeSettings {
    pub fn new(flavor: Flavor) -> Self {
        let s_max = match flavor {
            Flavor::Marlin => 255.0,
            Flavor::Smoothieware => 1.0,
            Flavor::LinuxCnc => 100.0,
            _ => 1000.0,
        };
        Self {
            flavor,
            s_max,
            decimals: 3,
            raster_decimals: 2,
            s_every_g1: false,
            overscan_as_g0: false,
            air: AirCommand::M8,
            marlin_power: MarlinPower::Inline,
            tool_index: None,
            analog_output: 0,
            start_script: String::new(),
            end_script: String::new(),
            variables: Vec::new(),
            comments: true,
        }
    }

    pub fn grbl() -> Self {
        Self::new(Flavor::Grbl)
    }
}

/// Write a toolpath as one G-code string (lines end with `\n`).
pub fn emit(tp: &Toolpath, s: &GcodeSettings) -> String {
    let mut out = String::new();
    for l in lines(tp, s) {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Stream G-code lines.
pub fn lines<'a>(tp: &'a Toolpath, s: &'a GcodeSettings) -> GcodeLines<'a> {
    GcodeLines { e: Emitter::new(tp, s), next_op: 0, phase: 0, buf: VecDeque::new() }
}

/// Iterator over G-code lines (without line endings).
pub struct GcodeLines<'a> {
    e: Emitter<'a>,
    next_op: usize,
    phase: u8,
    buf: VecDeque<String>,
}

impl Iterator for GcodeLines<'_> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        loop {
            if let Some(l) = self.buf.pop_front() {
                return Some(l);
            }
            let mut v = Vec::new();
            match self.phase {
                0 => {
                    self.e.header(&mut v);
                    self.phase = 1;
                }
                1 => {
                    if let Some(op) = self.e.tp.ops.get(self.next_op) {
                        self.e.op(op, &mut v);
                        self.next_op += 1;
                    } else {
                        self.phase = 2;
                    }
                }
                2 => {
                    self.e.footer(&mut v);
                    self.phase = 3;
                }
                _ => return None,
            }
            self.buf.extend(v);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Spindle {
    Off,
    Constant,
    Dynamic,
}

struct Emitter<'a> {
    tp: &'a Toolpath,
    s: &'a GcodeSettings,
    relative: bool,
    /// Position as written (rounded).
    pos: Point,
    z: Option<f64>,
    f: Option<f64>,
    /// Last S (or Q) value written.
    power: Option<f64>,
    spindle: Spindle,
    layer: Option<&'a IrLayer>,
    dec: usize,
    air: bool,
}

fn fmt(v: f64, dec: usize) -> String {
    let s = format!("{v:.dec$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

fn round(v: f64, dec: usize) -> f64 {
    let m = 10f64.powi(dec as i32);
    (v * m).round() / m
}

impl<'a> Emitter<'a> {
    fn new(tp: &'a Toolpath, s: &'a GcodeSettings) -> Self {
        let relative = tp.space == CoordSpace::RelativeToStart;
        Self {
            tp,
            s,
            relative,
            // Absolute jobs write both axes on the first move: the head may be anywhere.
            pos: if relative { Point::ZERO } else { Point::new(f64::NAN, f64::NAN) },
            z: None,
            f: None,
            power: None,
            spindle: Spindle::Off,
            layer: None,
            dec: s.decimals,
            air: false,
        }
    }

    fn comment(&self, text: &str, out: &mut Vec<String>) {
        if !self.s.comments {
            return;
        }
        if self.s.flavor == Flavor::LinuxCnc {
            out.push(format!("({})", text.replace(['(', ')'], "")));
        } else {
            out.push(format!("; {text}"));
        }
    }

    fn script(&self, script: &str, out: &mut Vec<String>) {
        for line in script.lines() {
            let mut l = line.replace("{flavor}", self.s.flavor.name()).replace("{s_max}", &fmt(self.s.s_max, 3));
            for (k, v) in &self.s.variables {
                l = l.replace(&format!("{{{k}}}"), v);
            }
            out.push(l);
        }
    }

    /// Power percent to S / Q value.
    fn s_value(&self, percent: f64) -> f64 {
        let v = (percent / 100.0).clamp(0.0, 1.0) * self.s.s_max;
        if self.s.s_max >= 100.0 { v.round() } else { round(v, 4) }
    }

    fn s_word(&self, v: f64) -> String {
        format!("S{}", fmt(v, 4))
    }

    fn header(&mut self, out: &mut Vec<String>) {
        self.comment(&format!("DarkBurn job for {}", self.s.flavor.name()), out);
        self.script(&self.s.start_script, out);
        out.push("G21".into());
        out.push(if self.relative { "G91".into() } else { "G90".into() });
        match self.s.flavor {
            Flavor::Marlin => {
                if let Some(t) = self.s.tool_index {
                    out.push(format!("T{t}"));
                }
                if self.s.marlin_power == MarlinPower::Inline {
                    out.push("M3 I S0".into());
                    self.power = Some(0.0);
                }
            }
            Flavor::LinuxCnc => {
                out.push(format!("M68 E{} Q0", self.s.analog_output));
                out.push("M3 S1".into());
                self.power = Some(0.0);
            }
            _ => {}
        }
        if self.relative {
            // Deltas need the finer grid so rounding never adds up.
            self.dec = self.s.decimals.max(self.s.raster_decimals);
        }
    }

    fn footer(&mut self, out: &mut Vec<String>) {
        if self.air {
            out.push("M9".into());
        }
        match self.s.flavor {
            f if f.is_grbl() => out.push("M5".into()),
            Flavor::Marlin => match self.s.marlin_power {
                MarlinPower::M106 => out.push("M107".into()),
                _ => out.push("M5".into()),
            },
            Flavor::LinuxCnc => {
                out.push(format!("M68 E{} Q0", self.s.analog_output));
                out.push("M5".into());
            }
            _ => {}
        }
        self.script(&self.s.end_script, out);
        if self.s.flavor.is_grbl() || self.s.flavor == Flavor::LinuxCnc {
            out.push("M2".into());
        }
    }

    /// X/Y words for a move to `to`; updates the written position. `None` if nothing moves.
    fn xy(&mut self, to: Point) -> Option<String> {
        let rx = round(to.x, self.dec);
        let ry = round(to.y, self.dec);
        let mut w = String::new();
        if self.relative {
            let dx = round(rx - self.pos.x, self.dec);
            let dy = round(ry - self.pos.y, self.dec);
            if dx != 0.0 {
                w.push_str(&format!("X{}", fmt(dx, self.dec)));
            }
            if dy != 0.0 {
                w.push_str(&format!("{}Y{}", if w.is_empty() { "" } else { " " }, fmt(dy, self.dec)));
            }
            self.pos = Point::new(round(self.pos.x + dx, self.dec), round(self.pos.y + dy, self.dec));
        } else {
            if rx != self.pos.x {
                w.push_str(&format!("X{}", fmt(rx, self.dec)));
            }
            if ry != self.pos.y {
                w.push_str(&format!("{}Y{}", if w.is_empty() { "" } else { " " }, fmt(ry, self.dec)));
            }
            self.pos = Point::new(rx, ry);
        }
        (!w.is_empty()).then_some(w)
    }

    fn feed(&mut self, speed_mm_s: f64) -> Option<String> {
        let f = round(speed_mm_s * 60.0, 1);
        if self.f != Some(f) {
            self.f = Some(f);
            Some(format!("F{}", fmt(f, 1)))
        } else {
            None
        }
    }

    fn wanted_spindle(&self) -> Spindle {
        if self.layer.is_some_and(|l| l.constant_power) { Spindle::Constant } else { Spindle::Dynamic }
    }

    /// GRBL: make sure M3/M4 matches the layer.
    fn grbl_mode(&mut self, want: Spindle, out: &mut Vec<String>) {
        if self.spindle != want {
            out.push(if want == Spindle::Constant { "M3".into() } else { "M4".into() });
            self.spindle = want;
        }
    }

    /// Power changes that must be written on their own line before a move (Marlin M3/M106,
    /// LinuxCNC M67). Returns the S word for flavours that put power on the move.
    fn power_for_move(&mut self, percent: f64, out: &mut Vec<String>) -> Option<String> {
        let v = self.s_value(percent);
        match self.s.flavor {
            f if f.is_grbl() || f == Flavor::Smoothieware => {
                if self.s.flavor.is_grbl() {
                    let want = self.wanted_spindle();
                    self.grbl_mode(want, out);
                }
                if self.power != Some(v) || self.s.s_every_g1 {
                    self.power = Some(v);
                    Some(self.s_word(v))
                } else {
                    None
                }
            }
            Flavor::Marlin => match self.s.marlin_power {
                MarlinPower::Inline => {
                    if self.power != Some(v) || self.s.s_every_g1 {
                        self.power = Some(v);
                        Some(self.s_word(v))
                    } else {
                        None
                    }
                }
                MarlinPower::M3 | MarlinPower::M106 => {
                    if self.power != Some(v) {
                        let m106 = self.s.marlin_power == MarlinPower::M106;
                        out.push(match (v > 0.0, m106) {
                            (true, false) => format!("M3 {}", self.s_word(v)),
                            (true, true) => format!("M106 {}", self.s_word(v)),
                            (false, false) => "M5".into(),
                            (false, true) => "M107".into(),
                        });
                        self.power = Some(v);
                    }
                    None
                }
            },
            Flavor::LinuxCnc => {
                if self.power != Some(v) {
                    out.push(format!("M67 E{} Q{}", self.s.analog_output, fmt(v, 4)));
                    self.power = Some(v);
                }
                None
            }
            _ => None,
        }
    }

    fn rapid(&mut self, to: Point, out: &mut Vec<String>) {
        // Laser off before travel where G0 does not switch it off by itself.
        let mut tail = String::new();
        match self.s.flavor {
            Flavor::Marlin => match self.s.marlin_power {
                MarlinPower::Inline if self.power != Some(0.0) => {
                    tail = " S0".into();
                    self.power = Some(0.0);
                }
                MarlinPower::M3 | MarlinPower::M106 if self.power.is_some_and(|p| p > 0.0) => {
                    out.push(if self.s.marlin_power == MarlinPower::M3 { "M5".into() } else { "M107".into() });
                    self.power = Some(0.0);
                }
                _ => {}
            },
            Flavor::LinuxCnc if self.power.is_some_and(|p| p > 0.0) => {
                out.push(format!("M67 E{} Q0", self.s.analog_output));
                self.power = Some(0.0);
            }
            _ => {}
        }
        if let Some(w) = self.xy(to) {
            out.push(format!("G0 {w}{tail}"));
        }
    }

    fn cut(&mut self, g: &str, to: Point, extra: Option<String>, speed: f64, power: f64, out: &mut Vec<String>) {
        let sw = self.power_for_move(power, out);
        let Some(w) = self.xy(to) else {
            // Nothing moves; keep power state but do not write an empty move.
            if sw.is_some() {
                self.power = None;
            }
            return;
        };
        let mut line = format!("{g} {w}");
        if let Some(e) = extra {
            line.push(' ');
            line.push_str(&e);
        }
        if let Some(f) = self.feed(speed) {
            line.push(' ');
            line.push_str(&f);
        }
        if let Some(s) = sw {
            line.push(' ');
            line.push_str(&s);
        }
        out.push(line);
    }

    fn dwell(&mut self, ms: f64, power: f64, out: &mut Vec<String>) {
        let p_word = match self.s.flavor {
            Flavor::Marlin | Flavor::Smoothieware => format!("G4 P{}", fmt(ms, 0)),
            _ => format!("G4 P{}", fmt(ms / 1000.0, 3)),
        };
        if power <= 0.0 {
            out.push(p_word);
            return;
        }
        let v = self.s_value(power);
        match self.s.flavor {
            f if f.is_grbl() => {
                // Laser mode only fires in G1 with M3 while standing still.
                self.grbl_mode(Spindle::Constant, out);
                let here = if self.relative || self.pos.x.is_nan() { "X0".to_string() } else { format!("X{}", fmt(self.pos.x, self.dec)) };
                let f = self.f.unwrap_or_else(|| round(self.layer.map_or(10.0, |l| l.speed) * 60.0, 1));
                self.f = Some(f);
                out.push(format!("G1 {here} F{} {}", fmt(f, 1), self.s_word(v)));
                out.push(p_word);
                out.push("S0".into());
                self.power = Some(0.0);
            }
            Flavor::Marlin => {
                let (on, off) = if self.s.marlin_power == MarlinPower::M106 { ("M106", "M107") } else { ("M3", "M5") };
                out.push(format!("{on} {}", self.s_word(v)));
                out.push(p_word);
                out.push(off.into());
                self.power = if self.s.marlin_power == MarlinPower::Inline { None } else { Some(0.0) };
            }
            Flavor::Smoothieware => {
                out.push(format!("fire {}", fmt(power, 1)));
                out.push(p_word);
                out.push("fire off".into());
            }
            Flavor::LinuxCnc => {
                out.push(format!("M67 E{} Q{}", self.s.analog_output, fmt(v, 4)));
                out.push(p_word);
                out.push(format!("M67 E{} Q0", self.s.analog_output));
                self.power = Some(0.0);
            }
            _ => out.push(p_word),
        }
    }

    fn op(&mut self, op: &Op, out: &mut Vec<String>) {
        match op {
            Op::LayerStart { layer } => {
                self.layer = self.tp.layers.get(*layer);
                if !self.relative {
                    let raster = self.layer.is_some_and(|l| l.raster);
                    self.dec = if raster { self.s.raster_decimals } else { self.s.decimals };
                }
                if self.s.flavor.is_grbl() {
                    let want = self.wanted_spindle();
                    if self.spindle != want {
                        out.push(format!("{} S0", if want == Spindle::Constant { "M3" } else { "M4" }));
                        self.spindle = want;
                        self.power = Some(0.0);
                    }
                }
            }
            Op::PassStart { pass } => {
                if self.layer.is_some_and(|l| l.passes > 1) {
                    self.comment(&format!("Pass {pass}"), out);
                }
            }
            Op::Rapid { to } => self.rapid(*to, out),
            Op::Cut { to, speed, power } => self.cut("G1", *to, None, *speed, *power, out),
            Op::Overscan { to, speed } => {
                if self.s.overscan_as_g0 {
                    self.rapid(*to, out);
                } else {
                    self.cut("G1", *to, None, *speed, 0.0, out);
                }
            }
            Op::Arc { to, center, clockwise, speed, power } => {
                let from = self.pos;
                // I/J from the written start point.
                let ij = format!("I{} J{}", fmt(center.x - from.x, self.dec), fmt(center.y - from.y, self.dec));
                let g = if *clockwise { "G2" } else { "G3" };
                self.cut(g, *to, Some(ij), *speed, *power, out);
            }
            Op::Dwell { ms, power } => self.dwell(*ms, *power, out),
            Op::Air { on } => {
                if *on != self.air {
                    out.push(match (on, self.s.air) {
                        (false, _) => "M9".into(),
                        (true, AirCommand::M8) => "M8".into(),
                        (true, AirCommand::M7) => "M7".into(),
                    });
                    self.air = *on;
                }
            }
            Op::Laser { index } => match (self.s.flavor, self.s.tool_index) {
                (Flavor::Marlin, Some(t)) => out.push(format!("T{}", t + index.saturating_sub(1))),
                _ => {
                    if *index != 1 {
                        self.comment(&format!("Laser {index} (no tool change on this controller)"), out);
                    }
                }
            },
            Op::ZMove { z } => {
                let zr = round(*z, self.s.decimals);
                let word = if self.relative {
                    let dz = round(zr - self.z.unwrap_or(0.0), self.s.decimals);
                    if dz == 0.0 && self.z.is_some() { None } else { Some(fmt(dz, self.s.decimals)) }
                } else if self.z != Some(zr) {
                    Some(fmt(zr, self.s.decimals))
                } else {
                    None
                };
                if let Some(w) = word {
                    out.push(format!("G0 Z{w}"));
                }
                self.z = Some(zr);
            }
            Op::Comment { text } => self.comment(text, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::CutMode;

    fn layer(constant: bool, raster: bool) -> IrLayer {
        IrLayer {
            dsp: Default::default(),
            dot: None,
            layer_index: 0,
            sublayer: 0,
            name: "C00".into(),
            mode: if raster { CutMode::Fill } else { CutMode::Line },
            speed: 10.0,
            max_power: 50.0,
            min_power: 0.0,
            passes: 1,
            constant_power: constant,
            air: false,
            laser: 1,
            raster,
        }
    }

    fn tp(ops: Vec<Op>, constant: bool) -> Toolpath {
        let mut all = vec![Op::LayerStart { layer: 0 }];
        all.extend(ops);
        Toolpath { space: CoordSpace::MachineAbsolute, layers: vec![layer(constant, false)], ops: all, start: Point::ZERO }
    }

    fn sq() -> Vec<Op> {
        vec![
            Op::Rapid { to: Point::new(1.0, 1.0) },
            Op::Cut { to: Point::new(11.0, 1.0), speed: 10.0, power: 50.0 },
            Op::Cut { to: Point::new(11.0, 11.0), speed: 10.0, power: 50.0 },
        ]
    }

    #[test]
    fn grbl_dynamic_and_constant() {
        let g = emit(&tp(sq(), false), &GcodeSettings { comments: false, ..GcodeSettings::grbl() });
        assert_eq!(g, "G21\nG90\nM4 S0\nG0 X1 Y1\nG1 X11 F600 S500\nG1 Y11\nM5\nM2\n");
        let g = emit(&tp(sq(), true), &GcodeSettings { comments: false, s_every_g1: true, ..GcodeSettings::grbl() });
        assert!(g.contains("M3 S0\n"));
        assert!(g.contains("G1 Y11 S500\n"));
    }

    #[test]
    fn marlin_modes() {
        let mut s = GcodeSettings { comments: false, ..GcodeSettings::new(Flavor::Marlin) };
        s.marlin_power = MarlinPower::M3;
        let g = emit(&tp(sq(), false), &s);
        assert!(g.contains("M3 S128\nG1 X11 F600\n"), "{g}");
        s.marlin_power = MarlinPower::M106;
        s.tool_index = Some(0);
        let g = emit(&tp(sq(), false), &s);
        assert!(g.starts_with("G21\nG90\nT0\n"));
        assert!(g.contains("M106 S128\n"));
        s.marlin_power = MarlinPower::Inline;
        let g = emit(&tp(sq(), false), &s);
        assert!(g.contains("G1 X11 F600 S128\n"));
    }

    #[test]
    fn smoothie_and_linuxcnc() {
        let g = emit(&tp(sq(), false), &GcodeSettings { comments: false, ..GcodeSettings::new(Flavor::Smoothieware) });
        assert!(g.contains("G1 X11 F600 S0.5\n"), "{g}");
        let g = emit(&tp(sq(), false), &GcodeSettings::new(Flavor::LinuxCnc));
        assert!(g.contains("M67 E0 Q50\nG1 X11 F600\n"), "{g}");
        assert!(g.starts_with("(DarkBurn job for LinuxCNC)\n"));
    }

    #[test]
    fn relative_moves_do_not_drift() {
        let mut ops = Vec::new();
        for k in 1..=30 {
            ops.push(Op::Cut { to: Point::new(k as f64 / 3.0, 0.0), speed: 10.0, power: 10.0 });
        }
        let mut t = tp(ops, false);
        t.space = CoordSpace::RelativeToStart;
        let g = emit(&t, &GcodeSettings { comments: false, ..GcodeSettings::grbl() });
        assert!(g.contains("G91\n"));
        let sum: f64 = g.lines().filter_map(|l| l.split(' ').find_map(|w| w.strip_prefix('X'))).map(|v| v.parse::<f64>().unwrap()).sum();
        assert!((sum - 10.0).abs() < 1e-9, "{sum}");
    }

    #[test]
    fn dwell_overscan_air_and_scripts() {
        let ops = vec![
            Op::Air { on: true },
            Op::Rapid { to: Point::new(1.0, 1.0) },
            Op::Dwell { ms: 250.0, power: 20.0 },
            Op::Overscan { to: Point::new(2.0, 1.0), speed: 100.0 },
        ];
        let mut s = GcodeSettings { comments: false, overscan_as_g0: true, air: AirCommand::M7, ..GcodeSettings::grbl() };
        s.start_script = "; start {flavor} {job}".into();
        s.variables.push(("job".into(), "test".into()));
        let g = emit(&tp(ops.clone(), false), &s);
        assert!(g.starts_with("; start GRBL test\n"), "{g}");
        assert!(g.contains("M7\n"));
        assert!(g.contains("M3\nG1 X1 F600 S200\nG4 P0.25\nS0\n"), "{g}");
        assert!(g.contains("G0 X2\n"));
        assert!(g.contains("M9\nM5\nM2\n"));
        s.overscan_as_g0 = false;
        let g = emit(&tp(ops, false), &s);
        assert!(g.contains("M4\nG1 X2 F6000\n"), "{g}");
    }

    #[test]
    fn arcs_and_streaming() {
        let ops = vec![
            Op::Rapid { to: Point::new(0.0, 0.0) },
            Op::Arc { to: Point::new(10.0, 0.0), center: Point::new(5.0, 0.0), clockwise: true, speed: 10.0, power: 10.0 },
        ];
        let t = tp(ops, false);
        let s = GcodeSettings { comments: false, ..GcodeSettings::grbl() };
        let g = emit(&t, &s);
        assert!(g.contains("G2 X10 I5 J0 F600 S100\n"), "{g}");
        let streamed: Vec<String> = lines(&t, &s).collect();
        assert_eq!(streamed.join("\n") + "\n", g);
    }
}
