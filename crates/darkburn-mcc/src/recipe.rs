//! Mlaser recipe files and layer tables: reading them into [`FibreCraft`] / [`Co2Craft`] and
//! writing crafts back.
//!
//! A recipe file (`Cutting parameters/<head>/<material>/<name>.xml` in the machine package) is a
//! parameter document ([`crate::paramxml`]) with one layer section, `PLayerParam11` for the fibre
//! head or `PCO2LayerParam11` for the CO2 head, holding one `GP` entry. A layer table (the
//! vendor's `BkLayerPara.xml`, and the layer part of a machine backup) holds `PLayerParam1` to
//! `PLayerParam11` and `PCO2LayerParam1` to `PCO2LayerParam11`. Section numbers are 1-based; slot
//! `n` is layer index `n - 1` of a [`CraftTable`].
//!
//! The `GP` entry of a fibre layer has 164 attributes, of which the craft model covers about 90;
//! the CO2 entry has 20 (recipe files) or 21 (layer tables, which add `CutFreq`). Every attribute
//! is kept, in file order, with its text as stored. Writing a craft back changes only the
//! attributes whose craft field changed, in the vendor's number form
//! ([`crate::paramxml::format_g17`]), so reading and writing an unchanged craft gives the same
//! bytes. An attribute missing from the file stays missing unless its value changed.
//!
//! Field mapping (fibre): `NoFollow` is the negation of [`FollowerSettings::follow`];
//! `SlowStart` with `SlowStartLength` and `SlowStartSpeed` is [`FibreCraft::slow_start`];
//! pierce stage `i` (0 to 4) is `DrillHeight{i}`, `DrillFocusPos{i}`,
//! `EnableFocusGradual{i}`/`FocusGradualEndPos{i}`/`FocusGradualTime{i}` (the focus ramp),
//! `DrillPower{i}`, `DrillFreq{i}`, `DrillPeakCurrent{i}`, `DrillGasType{i}`,
//! `DrillGasPressure{i}`, `DrillDelay{i}` (the pierce time),
//! `EnableGradualDrill{i}`/`GradualTime{i}`, `BeforeLaserOffDelay{i}` and
//! `AfterLaserOffDelay{i}`. All five stored stages are read; [`ProcessType::stages`] says how
//! many run. `LayerFileName` is the craft name, and the nozzle is taken from that name
//! ([`RecipeName`]). See `docs/research/mlaser-files.md` for the full list.

use crate::craft::{
    Co2Craft, Craft, CraftIssue, CraftTable, CurveNode, CutSettings, FibreCraft, FocusRamp, FollowerSettings, GasType, LAYERS_PER_HEAD, PierceStage,
    ProcessType, SlowStart, SmoothPierce, format_curve_nodes, parse_curve_nodes,
};
use crate::paramxml::{Attr, AttrList, Entry, MlaserError, ParamDoc, Section, parse_number};

/// Pierce stages stored per fibre layer.
pub const STORED_STAGES: usize = 5;
/// Slot the vendor uses for single recipe files.
pub const RECIPE_SLOT: usize = 11;
/// Root element of every parameter document.
pub const ROOT: &str = "ParameterRoot";
/// Name of the layer entry.
pub const LAYER_ENTRY: &str = "GP";
/// A flat curve, the vendor's default, written when a craft has an empty curve.
pub const FLAT_CURVE: &str = "0,100,100,100";
/// CO2 pulse frequency assumed when a CO2 recipe has no `CutFreq` (recipe files never do).
pub const CO2_DEFAULT_FREQ_HZ: f64 = 5000.0;

/// Which head a layer belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    /// Fibre head (`PLayerParam{n}`).
    Fibre,
    /// CO2 head (`PCO2LayerParam{n}`).
    Co2,
}

impl LayerKind {
    /// The section name of a 1-based slot.
    pub fn section_name(self, slot: usize) -> String {
        match self {
            LayerKind::Fibre => format!("PLayerParam{slot}"),
            LayerKind::Co2 => format!("PCO2LayerParam{slot}"),
        }
    }

    /// Kind and 1-based slot from a section name.
    pub fn from_section_name(name: &str) -> Option<(LayerKind, usize)> {
        let (kind, digits) = if let Some(d) = name.strip_prefix("PCO2LayerParam") {
            (LayerKind::Co2, d)
        } else {
            (LayerKind::Fibre, name.strip_prefix("PLayerParam")?)
        };
        if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let slot: usize = digits.parse().ok()?;
        (slot >= 1).then_some((kind, slot))
    }

    /// The kind of a craft.
    pub fn of(craft: &Craft) -> LayerKind {
        match craft {
            Craft::Fibre(_) => LayerKind::Fibre,
            Craft::Co2(_) => LayerKind::Co2,
        }
    }
}

/// Something noticed while reading a layer: a value that is missing, not a number or not a known
/// code. The craft field keeps its default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadNote {
    /// The attribute.
    pub attr: String,
    /// What was found.
    pub message: String,
}

/// One layer read from a document.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRecord {
    /// Head.
    pub kind: LayerKind,
    /// 1-based slot.
    pub slot: usize,
    /// The craft.
    pub craft: Craft,
    /// Reading notes.
    pub notes: Vec<ReadNote>,
}

/// Validate a craft of either head.
pub fn validate_craft(craft: &Craft) -> Vec<CraftIssue> {
    match craft {
        Craft::Fibre(c) => c.validate(),
        Craft::Co2(c) => c.validate(),
    }
}

// ---------------------------------------------------------------------------------------------
// Reading

struct Rd<'a> {
    e: &'a Entry,
    notes: Vec<ReadNote>,
}

impl Rd<'_> {
    fn note(&mut self, attr: &str, message: impl Into<String>) {
        self.notes.push(ReadNote { attr: attr.to_string(), message: message.into() });
    }

    fn num_inner(&mut self, key: &str, default: f64, required: bool) -> f64 {
        match self.e.get(key) {
            None => {
                if required {
                    self.note(key, format!("missing; {default} used"));
                }
                default
            }
            Some(v) => match parse_number(&v) {
                Some(x) => x,
                None => {
                    self.note(key, format!("{v:?} is not a number; {default} used"));
                    default
                }
            },
        }
    }

    fn req(&mut self, key: &str, default: f64) -> f64 {
        self.num_inner(key, default, true)
    }

    fn num(&mut self, key: &str, default: f64) -> f64 {
        self.num_inner(key, default, false)
    }

    fn code(&mut self, key: &str, required: bool) -> Option<i64> {
        match self.e.get(key) {
            None => {
                if required {
                    self.note(key, "missing; default used");
                }
                None
            }
            Some(v) => {
                let c = parse_number(&v).filter(|x| x.fract() == 0.0 && x.abs() < 1e9).map(|x| x as i64);
                if c.is_none() {
                    self.note(key, format!("{v:?} is not a whole number; default used"));
                }
                c
            }
        }
    }

    fn flag(&mut self, key: &str, default: bool) -> bool {
        self.code(key, false).map_or(default, |c| c != 0)
    }

    fn gas(&mut self, key: &str, required: bool) -> GasType {
        match self.code(key, required) {
            None => GasType::default(),
            Some(c) => match u8::try_from(c).ok().and_then(GasType::from_code) {
                Some(g) => g,
                None => {
                    self.note(key, format!("unknown gas code {c}; air used"));
                    GasType::default()
                }
            },
        }
    }

    fn text(&self, key: &str) -> String {
        self.e.get(key).unwrap_or_default()
    }

    fn curve(&mut self, key: &str) -> Vec<CurveNode> {
        let s = self.text(key);
        match parse_curve_nodes(&s) {
            Ok(v) => v,
            Err(e) => {
                self.note(key, format!("{e}; empty curve used"));
                Vec::new()
            }
        }
    }
}

fn stage_key(base: &str, i: usize) -> String {
    format!("{base}{i}")
}

/// Read a fibre `GP` entry.
pub fn fibre_from_entry(e: &Entry) -> (FibreCraft, Vec<ReadNote>) {
    let mut r = Rd { e, notes: Vec::new() };
    let dc = CutSettings::default();
    let df = FollowerSettings::default();
    let cut = CutSettings {
        height_mm: r.req("CutHeight", dc.height_mm),
        focus_mm: r.num("CutFocusPos", dc.focus_mm),
        speed_mm_s: r.req("CutSpeed", dc.speed_mm_s),
        power_pct: r.req("CutPower", dc.power_pct),
        freq_hz: r.req("CutFreq", dc.freq_hz),
        peak_current_pct: r.num("CutPeakCurrent", dc.peak_current_pct),
        gas: r.gas("CutGasType", true),
        gas_pressure_bar: r.req("CutAirPressure", dc.gas_pressure_bar),
        laser_on_delay_ms: r.num("LaserOnDelay", dc.laser_on_delay_ms),
        laser_off_before_delay_ms: r.num("LaserOffBeforeDelay", dc.laser_off_before_delay_ms),
        laser_off_after_delay_ms: r.num("LaserOffAfterDelay", dc.laser_off_after_delay_ms),
        up_height_mm: r.num("UpHeight", dc.up_height_mm),
    };
    let follower = FollowerSettings {
        follow: !r.flag("NoFollow", !df.follow),
        fixed_height_cut_pos_mm: r.num("AdvFixHeightCutPos", df.fixed_height_cut_pos_mm),
        short_distance_no_lift: r.flag("ShortDistNoUp", df.short_distance_no_lift),
        pre_drill: r.flag("PreDrill", df.pre_drill),
        pre_drill_no_lift: r.flag("PreDrillIsNotUp", df.pre_drill_no_lift),
        pierce_again_before_cut: r.flag("AfterPreDrillMustDrillBeforeCut", df.pierce_again_before_cut),
        with_film: r.flag("WithFilm", df.with_film),
    };
    let mut pierce = Vec::new();
    for i in 0..STORED_STAGES {
        if !e.has(&stage_key("DrillHeight", i)) {
            break;
        }
        let d = PierceStage::default();
        let k = |b: &str| stage_key(b, i);
        let focus_ramp = r
            .flag(&k("EnableFocusGradual"), false)
            .then(|| FocusRamp { end_focus_mm: r.num(&k("FocusGradualEndPos"), 0.0), time_ms: r.num(&k("FocusGradualTime"), d.duration_ms) });
        let height_mm = r.num(&k("DrillHeight"), d.height_mm);
        let focus_mm = r.num(&k("DrillFocusPos"), d.focus_mm);
        let power_pct = r.num(&k("DrillPower"), d.power_pct);
        let freq_hz = r.num(&k("DrillFreq"), d.freq_hz);
        let peak_current_pct = r.num(&k("DrillPeakCurrent"), d.peak_current_pct);
        let gas = r.gas(&k("DrillGasType"), false);
        let gas_pressure_bar = r.num(&k("DrillGasPressure"), d.gas_pressure_bar);
        let duration_ms = r.num(&k("DrillDelay"), d.duration_ms);
        let gradual_time_ms = r.flag(&k("EnableGradualDrill"), false).then(|| r.num(&k("GradualTime"), duration_ms));
        pierce.push(PierceStage {
            height_mm,
            focus_mm,
            focus_ramp,
            power_pct,
            freq_hz,
            peak_current_pct,
            gas,
            gas_pressure_bar,
            duration_ms,
            gradual_time_ms,
            before_laser_off_delay_ms: r.num(&k("BeforeLaserOffDelay"), d.before_laser_off_delay_ms),
            after_laser_off_delay_ms: r.num(&k("AfterLaserOffDelay"), d.after_laser_off_delay_ms),
        });
    }
    let process = match r.code("ManuType", true) {
        None => ProcessType::default(),
        Some(c) => match u8::try_from(c).ok().and_then(ProcessType::from_code) {
            Some(p) => p,
            None => {
                r.note("ManuType", format!("process code {c} is not known (0 to 4); direct cut used"));
                ProcessType::default()
            }
        },
    };
    let slow_start =
        r.flag("SlowStart", false).then(|| SlowStart { length_mm: r.num("SlowStartLength", 1.0), speed_mm_s: r.num("SlowStartSpeed", 10.0) });
    let smooth_pierce = r.flag("EnableSmoothPierce", false).then(|| SmoothPierce {
        height_mm: r.num("SmoothPierceDrillHeight", 5.0),
        time_ms: r.num("SmoothPierceDrillTime_ms", 100.0),
        focus_mm: r.num("SmoothPierceDrillFocusPos", 0.0),
        power_pct: r.num("SmoothPierceDrillPower", 50.0),
        freq_hz: r.num("SmoothPierceDrillFreq", 500.0),
        peak_current_pct: r.num("SmoothPierceDrillPeakCurrent", 100.0),
    });
    let name = r.text("LayerFileName");
    let craft = FibreCraft {
        nozzle: RecipeName::parse(&name).nozzle,
        name,
        note: r.text("Note"),
        no_manu: r.flag("NoManu", false),
        process,
        cut,
        pierce,
        follower,
        gas_flags: crate::craft::GasFlags {
            short_dist_keep_on: r.flag("ShortDistGasKeepOn", false),
            keep_on_during_job: r.flag("NoCloseGasInManu", false),
        },
        power_curve: r.curve("PWMCurveNodes"),
        freq_curve: r.curve("FreqCurveNodes"),
        dynamic_power: r.flag("PowerAdjustWithSpeed", false),
        dynamic_freq: r.flag("FreqAdjustWithSpeed", false),
        slow_start,
        smooth_pierce,
    };
    (craft, r.notes)
}

/// Read a CO2 `GP` entry. A missing `CutFreq` (recipe files have none) gives
/// [`CO2_DEFAULT_FREQ_HZ`] without a note.
pub fn co2_from_entry(e: &Entry) -> (Co2Craft, Vec<ReadNote>) {
    let mut r = Rd { e, notes: Vec::new() };
    let d = Co2Craft::default();
    let craft = Co2Craft {
        name: r.text("LayerFileName"),
        note: r.text("Note"),
        speed_mm_s: r.req("CutSpeed", d.speed_mm_s),
        duty_pct: r.req("CutDuty", d.duty_pct),
        freq_hz: r.num("CutFreq", CO2_DEFAULT_FREQ_HZ),
        gas: r.gas("CutGasType", true),
        gas_pressure_bar: r.req("CutAirPressure", d.gas_pressure_bar),
        laser_on_delay_ms: r.num("LaserOnDelay", d.laser_on_delay_ms),
        fixed_height_cut_pos_mm: r.num("AdvFixHeightCutPos", d.fixed_height_cut_pos_mm),
        gas_flags: crate::craft::GasFlags {
            short_dist_keep_on: r.flag("ShortDistGasKeepOn", false),
            keep_on_during_job: r.flag("NoCloseGasInManu", false),
        },
        power_curve: r.curve("PWMCurveNodes"),
        freq_curve: r.curve("FreqCurveNodes"),
        dynamic_power: r.flag("PowerAdjustWithSpeed", false),
        dynamic_freq: r.flag("FreqAdjustWithSpeed", false),
        slow_start: None,
    };
    let slow_start =
        r.flag("SlowStart", false).then(|| SlowStart { length_mm: r.num("SlowStartLength", 1.0), speed_mm_s: r.num("SlowStartSpeed", 10.0) });
    (Co2Craft { slow_start, ..craft }, r.notes)
}

/// Read the craft of an entry of the given kind.
pub fn craft_from_entry(kind: LayerKind, e: &Entry) -> (Craft, Vec<ReadNote>) {
    match kind {
        LayerKind::Fibre => {
            let (c, n) = fibre_from_entry(e);
            (Craft::Fibre(c), n)
        }
        LayerKind::Co2 => {
            let (c, n) = co2_from_entry(e);
            (Craft::Co2(c), n)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Writing

fn same(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

/// Writes only what changed. `old` is the craft read from the same entry; `None` means the old
/// value is not modelled (an option that was off), and the file's own value is compared instead.
struct Wr<'a> {
    e: &'a mut Entry,
}

impl Wr<'_> {
    fn num(&mut self, key: &str, old: Option<f64>, new: f64) -> Result<(), MlaserError> {
        let unchanged = match old {
            Some(o) => same(o, new),
            None => self.e.get_f64(key).is_some_and(|f| same(f, new)),
        };
        if unchanged { Ok(()) } else { self.e.set_f64(key, new) }
    }

    fn flag(&mut self, key: &str, old: Option<bool>, new: bool) {
        let unchanged = match old {
            Some(o) => o == new,
            None => self.e.get_bool(key) == Some(new),
        };
        if !unchanged {
            self.e.set_bool(key, new);
        }
    }

    fn code(&mut self, key: &str, old: Option<u8>, new: u8) {
        let unchanged = match old {
            Some(o) => o == new,
            None => self.e.get_i64(key) == Some(i64::from(new)),
        };
        if !unchanged {
            self.e.set(key, &new.to_string());
        }
    }

    fn text(&mut self, key: &str, old: &str, new: &str) {
        if old != new {
            self.e.set(key, new);
        }
    }

    fn curve(&mut self, key: &str, old: &[CurveNode], new: &[CurveNode]) {
        if old != new {
            let s = if new.is_empty() { FLAT_CURVE.to_string() } else { format_curve_nodes(new) };
            self.e.set(key, &s);
        }
    }

    fn slow_start(&mut self, old: Option<SlowStart>, new: Option<SlowStart>) -> Result<(), MlaserError> {
        self.flag("SlowStart", Some(old.is_some()), new.is_some());
        if let Some(n) = new {
            self.num("SlowStartLength", old.map(|o| o.length_mm), n.length_mm)?;
            self.num("SlowStartSpeed", old.map(|o| o.speed_mm_s), n.speed_mm_s)?;
        }
        Ok(())
    }
}

/// Write a fibre craft into a `GP` entry, changing only what differs from the craft the entry
/// holds now. The nozzle is not stored (it lives in the name). At most five pierce stages are
/// stored; stages the new craft does not list keep their stored values.
pub fn apply_fibre(e: &mut Entry, new: &FibreCraft) -> Result<(), MlaserError> {
    if new.pierce.len() > STORED_STAGES {
        return Err(MlaserError::new(format!("{} pierce stages; at most {STORED_STAGES} are stored", new.pierce.len())));
    }
    let (old, _) = fibre_from_entry(e);
    let mut w = Wr { e };
    let (o, n) = (&old.cut, &new.cut);
    w.flag("NoManu", Some(old.no_manu), new.no_manu);
    w.flag("NoFollow", Some(!old.follower.follow), !new.follower.follow);
    w.flag("ShortDistNoUp", Some(old.follower.short_distance_no_lift), new.follower.short_distance_no_lift);
    w.flag("ShortDistGasKeepOn", Some(old.gas_flags.short_dist_keep_on), new.gas_flags.short_dist_keep_on);
    w.flag("NoCloseGasInManu", Some(old.gas_flags.keep_on_during_job), new.gas_flags.keep_on_during_job);
    w.flag("PreDrill", Some(old.follower.pre_drill), new.follower.pre_drill);
    w.flag("WithFilm", Some(old.follower.with_film), new.follower.with_film);
    w.num("CutHeight", Some(o.height_mm), n.height_mm)?;
    w.num("CutFocusPos", Some(o.focus_mm), n.focus_mm)?;
    w.num("CutSpeed", Some(o.speed_mm_s), n.speed_mm_s)?;
    w.num("CutPower", Some(o.power_pct), n.power_pct)?;
    w.num("CutFreq", Some(o.freq_hz), n.freq_hz)?;
    w.num("CutPeakCurrent", Some(o.peak_current_pct), n.peak_current_pct)?;
    w.code("CutGasType", Some(o.gas.code()), n.gas.code());
    w.num("CutAirPressure", Some(o.gas_pressure_bar), n.gas_pressure_bar)?;
    w.num("LaserOnDelay", Some(o.laser_on_delay_ms), n.laser_on_delay_ms)?;
    w.num("LaserOffBeforeDelay", Some(o.laser_off_before_delay_ms), n.laser_off_before_delay_ms)?;
    w.num("LaserOffAfterDelay", Some(o.laser_off_after_delay_ms), n.laser_off_after_delay_ms)?;
    w.num("UpHeight", Some(o.up_height_mm), n.up_height_mm)?;
    w.slow_start(old.slow_start, new.slow_start)?;
    for (i, s) in new.pierce.iter().enumerate() {
        let os = old.pierce.get(i);
        let k = |b: &str| stage_key(b, i);
        w.num(&k("DrillHeight"), os.map(|x| x.height_mm), s.height_mm)?;
        w.num(&k("DrillFocusPos"), os.map(|x| x.focus_mm), s.focus_mm)?;
        w.flag(&k("EnableFocusGradual"), os.map(|x| x.focus_ramp.is_some()), s.focus_ramp.is_some());
        if let Some(r) = s.focus_ramp {
            let or = os.and_then(|x| x.focus_ramp);
            w.num(&k("FocusGradualEndPos"), or.map(|x| x.end_focus_mm), r.end_focus_mm)?;
            w.num(&k("FocusGradualTime"), or.map(|x| x.time_ms), r.time_ms)?;
        }
        w.num(&k("DrillPower"), os.map(|x| x.power_pct), s.power_pct)?;
        w.num(&k("DrillFreq"), os.map(|x| x.freq_hz), s.freq_hz)?;
        w.num(&k("DrillPeakCurrent"), os.map(|x| x.peak_current_pct), s.peak_current_pct)?;
        w.code(&k("DrillGasType"), os.map(|x| x.gas.code()), s.gas.code());
        w.num(&k("DrillGasPressure"), os.map(|x| x.gas_pressure_bar), s.gas_pressure_bar)?;
        w.num(&k("DrillDelay"), os.map(|x| x.duration_ms), s.duration_ms)?;
        w.flag(&k("EnableGradualDrill"), os.map(|x| x.gradual_time_ms.is_some()), s.gradual_time_ms.is_some());
        if let Some(g) = s.gradual_time_ms {
            w.num(&k("GradualTime"), os.and_then(|x| x.gradual_time_ms), g)?;
        }
        w.num(&k("BeforeLaserOffDelay"), os.map(|x| x.before_laser_off_delay_ms), s.before_laser_off_delay_ms)?;
        w.num(&k("AfterLaserOffDelay"), os.map(|x| x.after_laser_off_delay_ms), s.after_laser_off_delay_ms)?;
    }
    w.flag("PowerAdjustWithSpeed", Some(old.dynamic_power), new.dynamic_power);
    w.text("Note", &old.note, &new.note);
    w.curve("PWMCurveNodes", &old.power_curve, &new.power_curve);
    w.curve("FreqCurveNodes", &old.freq_curve, &new.freq_curve);
    w.code("ManuType", Some(old.process.code()), new.process.code());
    w.flag("PreDrillIsNotUp", Some(old.follower.pre_drill_no_lift), new.follower.pre_drill_no_lift);
    w.flag("AfterPreDrillMustDrillBeforeCut", Some(old.follower.pierce_again_before_cut), new.follower.pierce_again_before_cut);
    w.num("AdvFixHeightCutPos", Some(old.follower.fixed_height_cut_pos_mm), new.follower.fixed_height_cut_pos_mm)?;
    w.text("LayerFileName", &old.name, &new.name);
    w.flag("EnableSmoothPierce", Some(old.smooth_pierce.is_some()), new.smooth_pierce.is_some());
    if let Some(sp) = new.smooth_pierce {
        let o = old.smooth_pierce;
        w.num("SmoothPierceDrillHeight", o.map(|x| x.height_mm), sp.height_mm)?;
        w.num("SmoothPierceDrillTime_ms", o.map(|x| x.time_ms), sp.time_ms)?;
        w.num("SmoothPierceDrillFocusPos", o.map(|x| x.focus_mm), sp.focus_mm)?;
        w.num("SmoothPierceDrillPower", o.map(|x| x.power_pct), sp.power_pct)?;
        w.num("SmoothPierceDrillFreq", o.map(|x| x.freq_hz), sp.freq_hz)?;
        w.num("SmoothPierceDrillPeakCurrent", o.map(|x| x.peak_current_pct), sp.peak_current_pct)?;
    }
    w.flag("FreqAdjustWithSpeed", Some(old.dynamic_freq), new.dynamic_freq);
    Ok(())
}

/// Write a CO2 craft into a `GP` entry, changing only what differs.
pub fn apply_co2(e: &mut Entry, new: &Co2Craft) -> Result<(), MlaserError> {
    let (old, _) = co2_from_entry(e);
    let mut w = Wr { e };
    w.flag("PowerAdjustWithSpeed", Some(old.dynamic_power), new.dynamic_power);
    w.flag("FreqAdjustWithSpeed", Some(old.dynamic_freq), new.dynamic_freq);
    w.num("CutSpeed", Some(old.speed_mm_s), new.speed_mm_s)?;
    w.num("AdvFixHeightCutPos", Some(old.fixed_height_cut_pos_mm), new.fixed_height_cut_pos_mm)?;
    w.num("LaserOnDelay", Some(old.laser_on_delay_ms), new.laser_on_delay_ms)?;
    w.num("CutDuty", Some(old.duty_pct), new.duty_pct)?;
    w.code("CutGasType", Some(old.gas.code()), new.gas.code());
    w.num("CutAirPressure", Some(old.gas_pressure_bar), new.gas_pressure_bar)?;
    w.flag("ShortDistGasKeepOn", Some(old.gas_flags.short_dist_keep_on), new.gas_flags.short_dist_keep_on);
    w.flag("NoCloseGasInManu", Some(old.gas_flags.keep_on_during_job), new.gas_flags.keep_on_during_job);
    w.slow_start(old.slow_start, new.slow_start)?;
    w.curve("PWMCurveNodes", &old.power_curve, &new.power_curve);
    w.curve("FreqCurveNodes", &old.freq_curve, &new.freq_curve);
    w.text("LayerFileName", &old.name, &new.name);
    w.text("Note", &old.note, &new.note);
    w.num("CutFreq", Some(old.freq_hz), new.freq_hz)?;
    Ok(())
}

/// Write a craft of either head into an entry.
pub fn apply_craft(e: &mut Entry, craft: &Craft) -> Result<(), MlaserError> {
    match craft {
        Craft::Fibre(c) => apply_fibre(e, c),
        Craft::Co2(c) => apply_co2(e, c),
    }
}

// ---------------------------------------------------------------------------------------------
// Templates

fn attr(name: &str, raw: &str) -> Attr {
    Attr { name: name.to_string(), raw: raw.to_string() }
}

fn push_stage(v: &mut Vec<Attr>, i: usize) {
    for (base, raw) in [
        ("DrillHeight", "5"),
        ("DrillFocusPos", "0"),
        ("EnableFocusGradual", "0"),
        ("FocusGradualEndPos", "0"),
        ("FocusGradualTime", "1000"),
        ("DrillPower", "50"),
        ("DrillFreq", "500"),
        ("DrillPeakCurrent", "100"),
        ("DrillGasType", "0"),
        ("DrillGasPressure", "5"),
        ("DrillDelay", "1000"),
        ("EnableGradualDrill", "0"),
        ("GradualTime", "1000"),
        ("BeforeLaserOffDelay", "0"),
        ("AfterLaserOffDelay", "0"),
    ] {
        v.push(attr(&stage_key(base, i), raw));
    }
}

fn push_bolt(v: &mut Vec<Attr>, n: usize) {
    v.push(attr(&format!("BoltDrill_Enable_{n}"), "0"));
    v.push(attr(&format!("BoltDrill_Power_{n}"), "10"));
    v.push(attr(&format!("BoltDrill_Freq_{n}"), "1000"));
}

fn push_all(v: &mut Vec<Attr>, list: &[(&str, &str)]) {
    v.extend(list.iter().map(|(n, r)| attr(n, r)));
}

/// The 164 attributes of a fibre `GP` entry in the vendor's order, with the vendor's neutral
/// defaults (every option off, flat curves, five default pierce stages).
pub fn fibre_template() -> Entry {
    let mut v = Vec::with_capacity(164);
    push_all(
        &mut v,
        &[
            ("NoManu", "0"),
            ("NoFollow", "0"),
            ("ShortDistNoUp", "1"),
            ("ShortDistGasKeepOn", "0"),
            ("NoCloseGasInManu", "0"),
            ("PreDrill", "0"),
            ("WithFilm", "0"),
            ("CutHeight", "1"),
            ("CutFocusPos", "0"),
            ("CutSpeed", "100"),
            ("CutPower", "100"),
            ("CutFreq", "5000"),
            ("CutPeakCurrent", "100"),
            ("CutGasType", "0"),
            ("CutAirPressure", "5"),
            ("LaserOnDelay", "0"),
            ("LaserOffBeforeDelay", "0"),
            ("LaserOffAfterDelay", "0"),
            ("UpHeight", "20"),
            ("SlowStart", "0"),
            ("SlowStartLength", "1"),
            ("SlowStartSpeed", "10"),
            ("PreLaserOnFactor", "0"),
        ],
    );
    for i in 0..3 {
        push_stage(&mut v, i);
    }
    push_all(
        &mut v,
        &[
            ("PowerAdjustWithSpeed", "0"),
            ("Note", ""),
            ("PWMCurveNodes", FLAT_CURVE),
            ("FreqCurveNodes", FLAT_CURVE),
            ("ManuType", "0"),
            ("RecentFile", ""),
            ("PreDrillIsNotUp", "0"),
            ("AfterPreDrillMustDrillBeforeCut", "0"),
            ("AdvFixHeightCutPos", "20"),
            ("EnableContourShift", "0"),
            ("ContourShiftXDist", "0"),
            ("ContourShiftYDist", "0"),
        ],
    );
    for n in 1..=3 {
        push_bolt(&mut v, n);
    }
    push_all(
        &mut v,
        &[
            ("CleanResidue_Enable", "0"),
            ("CleanResidue_WorkH", "10"),
            ("CleanResidue_WorkV", "10"),
            ("CleanResidue_WorkFocus", "0"),
            ("CleanResidue_GasType", "0"),
            ("CleanResidue_GasP", "5"),
            ("CleanResidue_PeakCurrent", "50"),
            ("CleanResidue_Power", "10"),
            ("CleanResidue_Freq", "5000"),
            ("CleanResidue_WorkR", "2"),
            ("CleanResidue_SpiralTimes", "2"),
            ("UD_UpEnable", "0"),
            ("UD_UpLen", "5"),
            ("UD_UpSpeed", "8.3333333333333339"),
            ("UD_UpAdvEnable", "0"),
            ("UD_UpDuty", "10"),
            ("UD_UpFreq", "1000"),
            ("UD_DownEnable", "0"),
            ("UD_DownLen", "10"),
            ("UD_DownSpeed", "100"),
            ("UD_DownAdvEnable", "0"),
            ("UD_DownDuty", "10"),
            ("UD_DownFreq", "1000"),
            ("ZFVibAbat_Level", "0"),
            ("ZFVibAbat_Level_Thick", "0"),
            ("ZFVibAbat_Coef", "200"),
            ("LeadLineParam_Enable", "0"),
        ],
    );
    push_stage(&mut v, 3);
    push_bolt(&mut v, 4);
    push_stage(&mut v, 4);
    push_bolt(&mut v, 5);
    push_all(
        &mut v,
        &[
            ("ZFVibAbatType", "0"),
            ("LayerFileName", ""),
            ("EnableSmoothPierce", "0"),
            ("SmoothPierceDrillHeight", "5"),
            ("SmoothPierceDrillTime_ms", "100"),
            ("SmoothPierceDrillFocusPos", "0"),
            ("SmoothPierceDrillPower", "50"),
            ("SmoothPierceDrillFreq", "500"),
            ("SmoothPierceDrillPeakCurrent", "100"),
            ("PowerCurveSmoothType", "0"),
            ("FreqCurveSmoothType", "0"),
            ("FreqAdjustWithSpeed", "0"),
        ],
    );
    Entry { name: LAYER_ENTRY.to_string(), attrs: v }
}

/// The 20 attributes of a CO2 `GP` entry of a recipe file, in the vendor's order. A layer table
/// adds `CutFreq` at the end; [`apply_co2`] appends it when the frequency is not
/// [`CO2_DEFAULT_FREQ_HZ`].
pub fn co2_template() -> Entry {
    let mut v = Vec::with_capacity(21);
    push_all(
        &mut v,
        &[
            ("PowerCurveSmoothType", "0"),
            ("FreqCurveSmoothType", "0"),
            ("PowerAdjustWithSpeed", "0"),
            ("FreqAdjustWithSpeed", "0"),
            ("CutSpeed", "100"),
            ("AdvFixHeightCutPos", "0"),
            ("LaserOnDelay", "0"),
            ("CutDuty", "100"),
            ("CutGasType", "3"),
            ("CutAirPressure", "5"),
            ("ShortDistGasKeepOn", "0"),
            ("NoCloseGasInManu", "0"),
            ("SlowStart", "0"),
            ("SlowStartLength", "1"),
            ("SlowStartSpeed", "10"),
            ("PWMCurveNodes", FLAT_CURVE),
            ("FreqCurveNodes", FLAT_CURVE),
            ("LayerFileName", ""),
            ("Note", ""),
            ("RecentFile", ""),
        ],
    );
    Entry { name: LAYER_ENTRY.to_string(), attrs: v }
}

/// A new entry holding `craft`, built from the template of its head.
pub fn entry_for(craft: &Craft) -> Result<Entry, MlaserError> {
    let mut e = match craft {
        Craft::Fibre(_) => fibre_template(),
        Craft::Co2(_) => co2_template(),
    };
    apply_craft(&mut e, craft)?;
    Ok(e)
}

// ---------------------------------------------------------------------------------------------
// Recipe files

/// A single recipe file.
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeFile {
    /// The document, kept whole so writing back is faithful.
    pub doc: ParamDoc,
    /// The layer it holds.
    pub layer: LayerRecord,
}

fn layer_sections(doc: &ParamDoc) -> Vec<(usize, LayerKind, usize)> {
    doc.sections.iter().enumerate().filter_map(|(i, s)| LayerKind::from_section_name(&s.name).map(|(k, slot)| (i, k, slot))).collect()
}

/// Read a recipe file: a parameter document with exactly one layer section that has a `GP`
/// entry.
pub fn read_recipe(bytes: &[u8]) -> Result<RecipeFile, MlaserError> {
    let doc = ParamDoc::parse(bytes)?;
    recipe_from_doc(doc)
}

/// A recipe from a parsed document.
pub fn recipe_from_doc(doc: ParamDoc) -> Result<RecipeFile, MlaserError> {
    if doc.root != ROOT {
        return Err(MlaserError::new(format!("root element is <{}>, not <{ROOT}>", doc.root)));
    }
    let layers = layer_sections(&doc);
    let (idx, kind, slot) = match layers.as_slice() {
        [one] => *one,
        [] => return Err(MlaserError::new("no layer section (PLayerParamN or PCO2LayerParamN)")),
        many => return Err(MlaserError::new(format!("{} layer sections: a layer table, not a single recipe", many.len()))),
    };
    let entry = doc.sections.get(idx).and_then(|s| s.entry(LAYER_ENTRY)).ok_or_else(|| MlaserError::new("the layer section has no GP entry"))?;
    let (craft, mut notes) = craft_from_entry(kind, entry);
    if slot > LAYERS_PER_HEAD {
        notes.push(ReadNote { attr: kind.section_name(slot), message: format!("slot {slot} is above {LAYERS_PER_HEAD}") });
    }
    Ok(RecipeFile { layer: LayerRecord { kind, slot, craft, notes }, doc })
}

impl RecipeFile {
    /// A new recipe file in the vendor layout (slot 11) holding `craft`.
    pub fn new(craft: &Craft) -> Result<RecipeFile, MlaserError> {
        let kind = LayerKind::of(craft);
        let entry = entry_for(craft)?;
        let doc = ParamDoc {
            sections: vec![Section { name: kind.section_name(RECIPE_SLOT), attrs: Vec::new(), entries: vec![entry] }],
            ..ParamDoc::default()
        };
        recipe_from_doc(doc)
    }

    /// Replace the craft. The head must stay the same.
    pub fn set_craft(&mut self, craft: &Craft) -> Result<(), MlaserError> {
        let kind = LayerKind::of(craft);
        if kind != self.layer.kind {
            return Err(MlaserError::new(format!("the file holds a {:?} layer, not {kind:?}", self.layer.kind)));
        }
        let name = kind.section_name(self.layer.slot);
        let e = self.doc.section_mut(&name).and_then(|s| s.entry_mut(LAYER_ENTRY)).ok_or_else(|| MlaserError::new("layer entry is gone"))?;
        apply_craft(e, craft)?;
        let (c, notes) = craft_from_entry(kind, e);
        self.layer.craft = c;
        self.layer.notes = notes;
        Ok(())
    }

    /// The file bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.to_bytes()
    }
}

/// The bytes of a new recipe file holding `craft`.
pub fn write_recipe(craft: &Craft) -> Result<Vec<u8>, MlaserError> {
    Ok(RecipeFile::new(craft)?.to_bytes())
}

// ---------------------------------------------------------------------------------------------
// Layer tables

/// A document with layer sections: the vendor's `BkLayerPara.xml` or a machine backup.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerTable {
    /// The document, kept whole.
    pub doc: ParamDoc,
}

impl LayerTable {
    /// Parse a document.
    pub fn parse(bytes: &[u8]) -> Result<LayerTable, MlaserError> {
        Ok(LayerTable { doc: ParamDoc::parse(bytes)? })
    }

    /// Every layer section with a `GP` entry, in file order.
    pub fn layers(&self) -> Vec<LayerRecord> {
        layer_sections(&self.doc)
            .into_iter()
            .filter_map(|(i, kind, slot)| {
                let e = self.doc.sections.get(i)?.entry(LAYER_ENTRY)?;
                let (craft, notes) = craft_from_entry(kind, e);
                Some(LayerRecord { kind, slot, craft, notes })
            })
            .collect()
    }

    /// The layers as a craft table. Slots that are missing keep the default craft; slots above
    /// 11 are left out with a note.
    pub fn to_craft_table(&self) -> (CraftTable, Vec<ReadNote>) {
        let mut t = CraftTable::default();
        let mut notes = Vec::new();
        for l in self.layers() {
            let section = l.kind.section_name(l.slot);
            notes.extend(l.notes.into_iter().map(|n| ReadNote { attr: format!("{section}.{}", n.attr), message: n.message }));
            let i = l.slot - 1;
            match l.craft {
                Craft::Fibre(c) if i < t.fibre.len() => t.fibre[i] = c,
                Craft::Co2(c) if i < t.co2.len() => t.co2[i] = c,
                _ => notes.push(ReadNote { attr: section, message: format!("slot {} is above {LAYERS_PER_HEAD}; left out", l.slot) }),
            }
        }
        (t, notes)
    }

    /// Write a craft table back. Each layer changes only what differs. A slot with no section
    /// gets one (after the last section of its head) only when its craft is not the default.
    pub fn apply_craft_table(&mut self, table: &CraftTable) -> Result<(), MlaserError> {
        let fibre = table.fibre.iter().cloned().map(Craft::Fibre);
        let co2 = table.co2.iter().cloned().map(Craft::Co2);
        let fibre_default = Craft::Fibre(FibreCraft::default());
        let co2_default = Craft::Co2(Co2Craft::default());
        for (i, craft) in fibre.enumerate().chain(co2.enumerate()) {
            let kind = LayerKind::of(&craft);
            let name = kind.section_name(i + 1);
            if let Some(e) = self.doc.section_mut(&name).and_then(|s| s.entry_mut(LAYER_ENTRY)) {
                apply_craft(e, &craft)?;
                continue;
            }
            let default = if kind == LayerKind::Fibre { &fibre_default } else { &co2_default };
            if craft == *default {
                continue;
            }
            let mut entry = match kind {
                LayerKind::Fibre => fibre_template(),
                LayerKind::Co2 => {
                    let mut e = co2_template();
                    e.set("CutFreq", "5000");
                    e
                }
            };
            apply_craft(&mut entry, &craft)?;
            let section = Section { name, attrs: Vec::new(), entries: vec![entry] };
            let at = self
                .doc
                .sections
                .iter()
                .rposition(|s| LayerKind::from_section_name(&s.name).is_some_and(|(k, slot)| k == kind && slot <= i + 1))
                .map(|p| p + 1)
                .or_else(|| {
                    (kind == LayerKind::Fibre)
                        .then(|| {
                            self.doc.sections.iter().position(|s| LayerKind::from_section_name(&s.name).is_some_and(|(k, _)| k == LayerKind::Co2))
                        })
                        .flatten()
                })
                .unwrap_or(self.doc.sections.len());
            self.doc.sections.insert(at, section);
        }
        Ok(())
    }

    /// The file bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.doc.to_bytes()
    }
}

// ---------------------------------------------------------------------------------------------
// Names

/// Gas named in a recipe name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedGas {
    /// `air`.
    Air,
    /// `O2`.
    O2,
    /// `N2`.
    N2,
}

impl NamedGas {
    /// True if the vendor gas code is this gas, at low or high pressure. Air fed on the nitrogen
    /// line (code 5) is not counted; see [`crate::library`].
    pub fn matches(self, gas: GasType) -> bool {
        matches!(
            (self, gas),
            (NamedGas::Air, GasType::Air | GasType::HighAir)
                | (NamedGas::O2, GasType::O2 | GasType::HighO2)
                | (NamedGas::N2, GasType::N2 | GasType::HighN2)
        )
    }
}

/// What a vendor recipe name says, for example `Carbon 3.0mm  1.2D F+13 O2` or
/// `SS 1mm 2.0SD F-3N2 1Mpa`. The names are written by hand and irregular: spaces may be doubled,
/// the focus may be written `F -2`, the gas may be glued to the focus (`F-4air`) and the pressure
/// may be `1Mpa`, `1.0MPA` or `0.6 MPA`. Anything not found is `None`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RecipeName {
    /// Material thickness, mm (`3.0mm`).
    pub thickness_mm: Option<f64>,
    /// Nozzle code, letters upper-cased (`1.5S`, `2.0SD`, `4.D`).
    pub nozzle: Option<String>,
    /// Hand-set focus, mm (`F+12`, `F-3`, `F0`).
    pub focus_mm: Option<f64>,
    /// Gas (`O2`, `N2`, `air`).
    pub gas: Option<NamedGas>,
    /// Gas pressure as written, MPa (`1Mpa`).
    pub pressure_mpa: Option<f64>,
}

fn number_before(s: &str, end: usize) -> Option<f64> {
    let head = s.get(..end)?;
    let start = head.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').len();
    let digits = head.get(start..)?;
    if digits.is_empty() || !digits.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    parse_number(digits)
}

impl RecipeName {
    /// Parse a recipe name (a file stem or a `LayerFileName`).
    pub fn parse(name: &str) -> RecipeName {
        let lower = name.to_ascii_lowercase();
        let mut out = RecipeName::default();
        // Thickness: a number directly before "mm".
        let mut from = 0;
        while let Some(p) = lower.get(from..).and_then(|t| t.find("mm")).map(|p| p + from) {
            if let Some(v) = number_before(&lower, p) {
                out.thickness_mm = Some(v);
                break;
            }
            from = p + 2;
        }
        // Pressure: a number, optional blanks, "mpa".
        if let Some(p) = lower.find("mpa") {
            let head = lower.get(..p).unwrap_or("").trim_end();
            out.pressure_mpa = number_before(head, head.len());
        }
        let words: Vec<&str> = name.split_whitespace().collect();
        for (wi, w) in words.iter().enumerate() {
            let wl = w.to_ascii_lowercase();
            // Nozzle: digits, an optional fraction, then only S and D letters.
            let letters = wl.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
            if out.nozzle.is_none()
                && letters.len() < wl.len()
                && !letters.is_empty()
                && letters.chars().all(|c| c == 's' || c == 'd')
                && wl.starts_with(|c: char| c.is_ascii_digit())
            {
                out.nozzle = Some(w.to_ascii_uppercase());
                continue;
            }
            // Focus: "F" and a signed number, possibly in the next word, possibly with gas glued on.
            if out.focus_mm.is_none() && (wl.starts_with('f')) {
                let mut rest = wl[1..].to_string();
                if rest.is_empty()
                    && let Some(next) = words.get(wi + 1)
                {
                    rest = next.to_ascii_lowercase();
                }
                let sign_len = usize::from(rest.starts_with(['+', '-']));
                let num_len = rest[sign_len..].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len() - sign_len);
                if num_len > 0
                    && let Some(v) = parse_number(&rest[..sign_len + num_len])
                {
                    out.focus_mm = Some(v);
                    if let Some(g) = gas_word(&rest[sign_len + num_len..]) {
                        out.gas = Some(g);
                    }
                }
                continue;
            }
            if out.gas.is_none()
                && let Some(g) = gas_word(&wl)
            {
                out.gas = Some(g);
            }
        }
        out
    }
}

fn gas_word(w: &str) -> Option<NamedGas> {
    match w {
        "air" => Some(NamedGas::Air),
        "o2" => Some(NamedGas::O2),
        "n2" => Some(NamedGas::N2),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::craft::Severity;

    fn fibre_sample() -> FibreCraft {
        let mut c = FibreCraft::default();
        c.name = "Demo 3mm 1.5S F+2 O2".into();
        c.nozzle = Some("1.5S".into());
        c.process = ProcessType::Pierce2;
        c.cut.speed_mm_s = 2000.0 / 60.0;
        c.cut.gas = GasType::O2;
        c.cut.gas_pressure_bar = 0.6;
        c.cut.laser_on_delay_ms = 250.0;
        c.pierce = vec![
            PierceStage { height_mm: 8.0, gas: GasType::O2, duration_ms: 300.0, gradual_time_ms: Some(300.0), ..PierceStage::default() },
            PierceStage { height_mm: 4.0, focus_ramp: Some(FocusRamp { end_focus_mm: -1.0, time_ms: 200.0 }), ..PierceStage::default() },
            PierceStage::default(),
            PierceStage::default(),
            PierceStage::default(),
        ];
        c.power_curve = vec![CurveNode { speed_pct: 0.0, output_pct: 20.0 }, CurveNode { speed_pct: 100.0, output_pct: 100.0 }];
        c.freq_curve = vec![CurveNode { speed_pct: 0.0, output_pct: 100.0 }, CurveNode { speed_pct: 100.0, output_pct: 100.0 }];
        c.dynamic_power = true;
        c.slow_start = Some(SlowStart { length_mm: 2.0, speed_mm_s: 12.5 });
        c
    }

    #[test]
    fn templates_have_the_vendor_attribute_counts() {
        assert_eq!(fibre_template().attrs.len(), 164);
        assert_eq!(co2_template().attrs.len(), 20);
        let names: std::collections::HashSet<_> = fibre_template().attrs.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names.len(), 164, "attribute names are unique");
    }

    #[test]
    fn the_template_reads_as_the_default_craft() {
        let (c, notes) = fibre_from_entry(&fibre_template());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(c.cut, CutSettings::default());
        assert_eq!(c.follower, FollowerSettings::default());
        assert_eq!(c.pierce, vec![PierceStage::default(); 5]);
        let (c, notes) = co2_from_entry(&co2_template());
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(c.freq_hz, CO2_DEFAULT_FREQ_HZ);
    }

    #[test]
    fn a_new_fibre_recipe_reads_back_to_the_same_craft() {
        let c = fibre_sample();
        let bytes = write_recipe(&Craft::Fibre(c.clone())).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("<ParameterRoot>\r\n<PLayerParam11>\r\n<GP NoManu=\"0\" NoFollow=\"0\""));
        assert!(text.ends_with("/>\r\n</PLayerParam11>\r\n</ParameterRoot>\r\n"));
        assert!(text.contains(" CutSpeed=\"33.333333333333336\" "));
        assert!(text.contains(" CutAirPressure=\"0.59999999999999998\" "));
        assert!(text.contains(" PWMCurveNodes=\"0,20,100,100\" "));
        assert!(text.contains(" ManuType=\"3\" "));
        assert!(text.contains(" EnableFocusGradual1=\"1\" FocusGradualEndPos1=\"-1\" FocusGradualTime1=\"200\" "));
        let r = read_recipe(&bytes).unwrap();
        assert_eq!(r.layer.kind, LayerKind::Fibre);
        assert_eq!(r.layer.slot, 11);
        assert!(r.layer.notes.is_empty());
        assert_eq!(r.layer.craft, Craft::Fibre(c));
        assert_eq!(r.to_bytes(), bytes, "write of what was read is byte-identical");
    }

    #[test]
    fn a_new_co2_recipe_reads_back_and_has_no_cut_freq_by_default() {
        let mut c = Co2Craft::default();
        c.name = "Acrylic 3mm air".into();
        c.speed_mm_s = 25.0;
        c.duty_pct = 60.0;
        c.gas = GasType::HighAir;
        // An empty curve is written as the flat vendor curve, so give the craft that curve.
        c.power_curve = parse_curve_nodes(FLAT_CURVE).unwrap();
        c.freq_curve = c.power_curve.clone();
        let bytes = write_recipe(&Craft::Co2(c.clone())).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("<ParameterRoot>\r\n<PCO2LayerParam11>\r\n<GP PowerCurveSmoothType=\"0\""));
        assert!(!text.contains("CutFreq"));
        let r = read_recipe(&bytes).unwrap();
        assert_eq!(r.layer.craft, Craft::Co2(c.clone()));
        c.freq_hz = 2000.0;
        let text = String::from_utf8(write_recipe(&Craft::Co2(c)).unwrap()).unwrap();
        assert!(text.contains(" RecentFile=\"\" CutFreq=\"2000\"/>"));
    }

    /// A hand-written recipe with odd number forms and extra attributes the model does not know.
    const ODD: &str = "<ParameterRoot>\r\n<PLayerParam11>\r\n<GP Mystery=\"x&amp;y\" CutHeight=\"1.0\" CutSpeed=\"41.666666666666671\" CutPower=\"100\" CutFreq=\"5000\" CutGasType=\"5\" CutAirPressure=\"1\" ManuType=\"0\" LayerFileName=\"Demo 1mm 2.0S F0 air\" Tail=\"7\"/>\r\n</PLayerParam11>\r\n</ParameterRoot>\r\n";

    #[test]
    fn unknown_attributes_and_number_forms_survive_a_round_trip() {
        let mut r = read_recipe(ODD.as_bytes()).unwrap();
        assert_eq!(r.to_bytes(), ODD.as_bytes());
        let craft = r.layer.craft.clone();
        r.set_craft(&craft).unwrap();
        assert_eq!(r.to_bytes(), ODD.as_bytes(), "an unchanged craft changes nothing");
        let Craft::Fibre(mut c) = craft else { panic!("fibre") };
        assert_eq!(c.cut.gas, GasType::HighN2);
        assert_eq!(c.nozzle.as_deref(), Some("2.0S"));
        c.cut.speed_mm_s = 50.0;
        r.set_craft(&Craft::Fibre(c)).unwrap();
        let text = String::from_utf8(r.to_bytes()).unwrap();
        assert_eq!(text, ODD.replace("41.666666666666671", "50"));
    }

    #[test]
    fn options_switched_on_write_their_values_and_off_keeps_them() {
        let mut r = read_recipe(ODD.as_bytes()).unwrap();
        let Craft::Fibre(mut c) = r.layer.craft.clone() else { panic!("fibre") };
        c.slow_start = Some(SlowStart { length_mm: 3.0, speed_mm_s: 10.0 });
        c.smooth_pierce =
            Some(SmoothPierce { height_mm: 4.0, time_ms: 100.0, focus_mm: 0.0, power_pct: 50.0, freq_hz: 500.0, peak_current_pct: 100.0 });
        r.set_craft(&Craft::Fibre(c.clone())).unwrap();
        let text = String::from_utf8(r.to_bytes()).unwrap();
        assert!(text.contains(" SlowStart=\"1\" SlowStartLength=\"3\" SlowStartSpeed=\"10\""));
        assert!(text.contains(" EnableSmoothPierce=\"1\" SmoothPierceDrillHeight=\"4\" SmoothPierceDrillTime_ms=\"100\""));
        assert_eq!(r.layer.craft, Craft::Fibre(c.clone()));
        c.slow_start = None;
        r.set_craft(&Craft::Fibre(c)).unwrap();
        let text = String::from_utf8(r.to_bytes()).unwrap();
        assert!(text.contains(" SlowStart=\"0\" SlowStartLength=\"3\""));
    }

    #[test]
    fn bad_values_give_notes_and_defaults() {
        let s = "<ParameterRoot><PLayerParam11><GP CutSpeed=\"nan\" CutPower=\"1e999\" CutFreq=\"abc\" CutGasType=\"9\" ManuType=\"7\" CutHeight=\"2.5\" DrillHeight0=\"x\" PWMCurveNodes=\"1,2,3\"/></PLayerParam11></ParameterRoot>";
        let r = read_recipe(s.as_bytes()).unwrap();
        let attrs: Vec<&str> = r.layer.notes.iter().map(|n| n.attr.as_str()).collect();
        for a in ["CutSpeed", "CutPower", "CutFreq", "CutGasType", "ManuType", "CutAirPressure", "DrillHeight0", "PWMCurveNodes"] {
            assert!(attrs.contains(&a), "{a} in {attrs:?}");
        }
        let Craft::Fibre(c) = &r.layer.craft else { panic!("fibre") };
        assert_eq!(c.cut.speed_mm_s, CutSettings::default().speed_mm_s);
        assert_eq!(c.cut.height_mm, 2.5);
        assert_eq!(c.process, ProcessType::DirectCut);
        assert_eq!(c.pierce.len(), 1);
        // Unchanged write-back leaves the bad text alone.
        let mut r2 = r.clone();
        r2.set_craft(&r.layer.craft).unwrap();
        assert_eq!(r2.to_bytes(), r.to_bytes());
    }

    #[test]
    fn non_finite_values_are_refused_on_write() {
        let mut c = fibre_sample();
        c.cut.power_pct = f64::NAN;
        assert!(write_recipe(&Craft::Fibre(c.clone())).is_err());
        c.cut.power_pct = 100.0;
        c.pierce.push(PierceStage::default());
        assert!(write_recipe(&Craft::Fibre(c)).is_err(), "six stages");
    }

    #[test]
    fn a_recipe_needs_exactly_one_layer() {
        assert!(read_recipe(b"<ParameterRoot>\r\n</ParameterRoot>\r\n").is_err());
        assert!(read_recipe(b"<Other><PLayerParam11><GP/></PLayerParam11></Other>").is_err());
        assert!(read_recipe(b"<ParameterRoot><PLayerParam1><GP/></PLayerParam1><PLayerParam2><GP/></PLayerParam2></ParameterRoot>").is_err());
        assert!(read_recipe(b"<ParameterRoot><PLayerParam11><XX/></PLayerParam11></ParameterRoot>").is_err());
        let mut r = read_recipe(b"<ParameterRoot><PLayerParam11><GP/></PLayerParam11></ParameterRoot>").unwrap();
        assert!(r.set_craft(&Craft::Co2(Co2Craft::default())).is_err(), "head may not change");
    }

    #[test]
    fn section_names() {
        assert_eq!(LayerKind::from_section_name("PLayerParam11"), Some((LayerKind::Fibre, 11)));
        assert_eq!(LayerKind::from_section_name("PCO2LayerParam3"), Some((LayerKind::Co2, 3)));
        assert_eq!(LayerKind::from_section_name("PLayerParam0"), None);
        assert_eq!(LayerKind::from_section_name("PLayerParam"), None);
        assert_eq!(LayerKind::from_section_name("PLayerParamX"), None);
        assert_eq!(LayerKind::from_section_name("PLayerParam99999999999999999999"), None);
        assert_eq!(LayerKind::Co2.section_name(2), "PCO2LayerParam2");
    }

    fn table_doc() -> String {
        let mut s = String::from("<ParameterRoot>\r\n");
        for (kind, n) in [(LayerKind::Fibre, 1), (LayerKind::Fibre, 2), (LayerKind::Co2, 1)] {
            let mut e = if kind == LayerKind::Fibre { fibre_template() } else { co2_template() };
            if kind == LayerKind::Co2 {
                e.set("CutFreq", "5000");
            }
            e.set("LayerFileName", &format!("layer {n}"));
            let doc = ParamDoc { sections: vec![Section { name: kind.section_name(n), attrs: vec![], entries: vec![e] }], ..ParamDoc::default() };
            let t = doc.to_text();
            let inner: Vec<&str> = t.split("\r\n").collect();
            for l in &inner[1..inner.len() - 2] {
                s.push_str(l);
                s.push_str("\r\n");
            }
        }
        s.push_str("</ParameterRoot>\r\n");
        s
    }

    #[test]
    fn layer_table_round_trip_and_edit() {
        let src = table_doc();
        let mut t = LayerTable::parse(src.as_bytes()).unwrap();
        assert_eq!(t.layers().len(), 3);
        let (mut table, notes) = t.to_craft_table();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(table.fibre[1].name, "layer 2");
        assert_eq!(table.co2[0].name, "layer 1");
        t.apply_craft_table(&table).unwrap();
        assert_eq!(t.to_bytes(), src.as_bytes(), "an unchanged table changes nothing");
        table.fibre[1].cut.power_pct = 80.0;
        table.fibre[4] = FibreCraft { name: "new five".into(), ..fibre_from_entry(&fibre_template()).0 };
        table.co2[0].freq_hz = 1000.0;
        t.apply_craft_table(&table).unwrap();
        let names: Vec<&str> = t.doc.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["PLayerParam1", "PLayerParam2", "PLayerParam5", "PCO2LayerParam1"]);
        let (back, _) = t.to_craft_table();
        assert_eq!(back.fibre[1].cut.power_pct, 80.0);
        assert_eq!(back.fibre[4].name, "new five");
        assert_eq!(back.co2[0].freq_hz, 1000.0);
        assert_eq!(back, table);
    }

    #[test]
    fn recipe_names() {
        let n = RecipeName::parse("Carbon 3.0mm  1.2D F+13 O2");
        assert_eq!(n.thickness_mm, Some(3.0));
        assert_eq!(n.nozzle.as_deref(), Some("1.2D"));
        assert_eq!(n.focus_mm, Some(13.0));
        assert_eq!(n.gas, Some(NamedGas::O2));
        assert_eq!(n.pressure_mpa, None);
        let n = RecipeName::parse("SS 1mm 2.0SD F-3N2 1Mpa");
        assert_eq!(
            (n.thickness_mm, n.nozzle.as_deref(), n.focus_mm, n.gas, n.pressure_mpa),
            (Some(1.0), Some("2.0SD"), Some(-3.0), Some(NamedGas::N2), Some(1.0))
        );
        let n = RecipeName::parse("Al 2mm 1.5s F -2 air");
        assert_eq!((n.nozzle.as_deref(), n.focus_mm, n.gas), (Some("1.5S"), Some(-2.0), Some(NamedGas::Air)));
        let n = RecipeName::parse("Cu 4mm 4.D F-4air");
        assert_eq!((n.nozzle.as_deref(), n.focus_mm, n.gas), (Some("4.D"), Some(-4.0), Some(NamedGas::Air)));
        let n = RecipeName::parse("Cleaning  1.0MPA");
        assert_eq!((n.thickness_mm, n.pressure_mpa), (None, Some(1.0)));
        let n = RecipeName::parse("quenching  0.6 MPA");
        assert_eq!(n.pressure_mpa, Some(0.6));
        let n = RecipeName::parse("Acrylic 8mm air");
        assert_eq!((n.thickness_mm, n.gas, n.nozzle), (Some(8.0), Some(NamedGas::Air), None));
        for odd in ["", "F", "F+", "mm", "MPA", "....mm", "F.", "\u{4e0d}\u{9508}\u{94a2}", "f-", "1e999mm", "F1e999"] {
            let _ = RecipeName::parse(odd);
        }
    }

    #[test]
    fn named_gas_matches_codes() {
        assert!(NamedGas::O2.matches(GasType::HighO2));
        assert!(NamedGas::N2.matches(GasType::N2));
        assert!(NamedGas::Air.matches(GasType::HighAir));
        assert!(!NamedGas::Air.matches(GasType::HighN2));
    }

    #[test]
    fn a_written_recipe_passes_the_craft_checks() {
        let r = RecipeFile::new(&Craft::Fibre(fibre_sample())).unwrap();
        let issues = validate_craft(&r.layer.craft);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
    }
}
