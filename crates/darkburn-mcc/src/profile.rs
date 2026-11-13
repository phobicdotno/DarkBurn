//! Mlaser machine settings (`BkHardPara.xml`, `BkManuPara.xml`, `BkLayerPara.xml` and the full
//! machine backup) read into one [`MachineProfile`].
//!
//! The vendor program splits its settings over the three `Bk*Para.xml` files; the backup holds
//! all of them in one document. Some entries are split too (half of `PSoftParam/SOP` is in each
//! of two files), so documents are merged attribute by attribute, keyed by section, entry name and
//! the entry's position among same-named entries. The first source to name an attribute wins; a
//! later source with a different value is recorded as a [`Conflict`].
//!
//! Everything is kept: the typed views below read the parts this program understands, and every
//! other section and attribute stays in [`MachineProfile::doc`] as written. Nothing here writes to
//! a machine, and the typed views only read: the follower, pierce and gas settings are shown, never
//! changed.
//!
//! Port numbers are the vendor's (`0` means "not wired"). Units are millimetres and mm/s where the
//! values make that plain; `docs/research/mlaser-files.md` lists what is inferred.

use std::collections::BTreeMap;

use crate::command::MachineParams;
use crate::craft::CraftTable;
use crate::mlaser_files::CompensationTable;
use crate::paramxml::{Attr, AttrList, Entry, MlaserError, ParamDoc, Section, parse_number};
use crate::recipe::{LayerKind, LayerTable, ReadNote};

/// Where an attribute lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttrPath {
    /// Section element (`PManuParam`).
    pub section: String,
    /// Entry element (`MC`).
    pub entry: String,
    /// Position among entries of that name in the section, from 0.
    pub occurrence: usize,
    /// Attribute.
    pub attr: String,
}

impl std::fmt::Display for AttrPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.occurrence == 0 {
            write!(f, "{}/{}.{}", self.section, self.entry, self.attr)
        } else {
            write!(f, "{}/{}[{}].{}", self.section, self.entry, self.occurrence, self.attr)
        }
    }
}

/// Two sources disagreeing about one attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The attribute.
    pub path: AttrPath,
    /// The value kept.
    pub kept: String,
    /// Source of the kept value.
    pub kept_from: String,
    /// The value not used.
    pub other: String,
    /// Source of the value not used.
    pub other_from: String,
}

/// Merged machine settings.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MachineProfile {
    /// The merged document: every section, entry and attribute of every source.
    pub doc: ParamDoc,
    /// Source labels in merge order.
    pub sources: Vec<String>,
    /// Attributes the sources disagree about.
    pub conflicts: Vec<Conflict>,
    /// Index into `sources` of the source each kept attribute came from.
    origins: BTreeMap<AttrPath, usize>,
}

/// True if two attribute texts mean the same: equal numbers, or equal text.
pub fn same_value(a: &str, b: &str) -> bool {
    match (parse_number(a), parse_number(b)) {
        (Some(x), Some(y)) => x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs()),
        _ => a == b,
    }
}

fn occurrence_of(section: &Section, index: usize) -> usize {
    let name = section.entries.get(index).map(|e| e.name.as_str()).unwrap_or("");
    section.entries.iter().take(index).filter(|e| e.name == name).count()
}

fn nth_entry<'a>(section: &'a Section, name: &str, occurrence: usize) -> Option<&'a Entry> {
    section.entries.iter().filter(|e| e.name == name).nth(occurrence)
}

/// Sections the typed views read; the rest are only kept.
pub const TYPED_SECTIONS: [&str; 10] =
    ["PAxisParam", "PHomeParam", "PZFParam", "PLaserParam", "PManuParam", "PGasParam", "PDOParam", "PDIParam", "PFCParam", "PGraphParam"];

/// One `name#port#...` item of the vendor's input and output lists (`Door#4#-1#0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedPort {
    /// Name as written (may be Chinese).
    pub name: String,
    /// Port; `0` or `None` means not wired.
    pub port: Option<i64>,
    /// The other `#` fields, meaning open.
    pub rest: Vec<String>,
}

/// Parse a `name#port#...,name#port#...` list.
pub fn parse_named_ports(s: &str) -> Vec<NamedPort> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .take(1000)
        .map(|p| {
            let mut f = p.split('#');
            let name = f.next().unwrap_or("").to_string();
            let port = f.next().and_then(parse_number).filter(|v| v.fract() == 0.0 && v.abs() < 1e9).map(|v| v as i64);
            NamedPort { name, port, rest: f.map(str::to_string).collect() }
        })
        .collect()
}

/// `PAxisParam/A<n>`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AxisParam {
    /// The `n` of `A<n>`.
    pub slot: usize,
    /// `EnableType` (code, meaning open; `2` on the fourth axis of the reference machine).
    pub enable_type: Option<i64>,
    /// `AxisIndex`.
    pub axis_index: Option<i64>,
    /// `DoubleDriver`: two motors on this axis.
    pub double_driver: Option<bool>,
    /// `PulseEquivalent`.
    pub pulse_equivalent: Option<f64>,
    /// `MaxLength`, mm.
    pub max_length_mm: Option<f64>,
    /// `LimitSwitchType`.
    pub limit_switch_type: Option<i64>,
    /// `EncoderReverse`.
    pub encoder_reverse: Option<bool>,
    /// `PHomeParam/A<n>.HomeDirection` (or `HPA<n>`).
    pub home_direction: Option<i64>,
    /// `PHomeParam/A<n>.HomeOffset`, mm.
    pub home_offset_mm: Option<f64>,
}

/// `PMachineAxisConfig_<n>/MAC` or `MAC_<n>`: the servo side of an axis. How these numbers map
/// onto the `A<n>` numbers of [`AxisParam`] is not known.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AxisConfig {
    /// The `n` of `PMachineAxisConfig_<n>`.
    pub slot: usize,
    /// `SoftLimitMaxLen`, mm: the travel the soft limit allows.
    pub soft_limit_max_len_mm: Option<f64>,
    /// `WritePluse` (sic): pulses per motor turn, inferred.
    pub write_pulse: Option<f64>,
    /// `SpeedRatio`: mm per motor turn, inferred.
    pub speed_ratio: Option<f64>,
    /// `AxisReverse`.
    pub axis_reverse: Option<bool>,
    /// `EncoderReverse`.
    pub encoder_reverse: Option<bool>,
    /// `IsRotatingShaft`: a rotary axis.
    pub rotating: Option<bool>,
    /// `NegativeLimitInput`, input port.
    pub negative_limit_input: Option<i64>,
    /// `ForwardLimitInput`, input port.
    pub forward_limit_input: Option<i64>,
    /// `FastSpeed`: homing search speed, mm/s.
    pub home_fast_speed: Option<f64>,
    /// `SecondSpeed`: homing second-pass speed, mm/s.
    pub home_second_speed: Option<f64>,
    /// `ReturnLength`, mm.
    pub return_length_mm: Option<f64>,
    /// `Acceleration`, mm/s².
    pub acceleration: Option<f64>,
}

/// `PHomeParam/HP` and `HPA3`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct HomeSettings {
    /// `UseZPulse`.
    pub use_z_pulse: Option<bool>,
    /// `SampleSignal`.
    pub sample_signal: Option<i64>,
    /// `LimitSwitchType`.
    pub limit_switch_type: Option<i64>,
    /// `FastSpeed`, mm/s.
    pub fast_speed: Option<f64>,
    /// `SlowSpeed`, mm/s.
    pub slow_speed: Option<f64>,
    /// `EnableSecondTimeHome`.
    pub second_pass: Option<bool>,
}

/// `PLaserParam/LGP` and `LPF`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LaserSettings {
    /// `LaserType`.
    pub fibre_type: Option<i64>,
    /// `LaserControlType`.
    pub fibre_control_type: Option<i64>,
    /// `LaserDAPort`: analogue output for fibre power.
    pub fibre_da_port: Option<i64>,
    /// `DOLaserGate`: output port.
    pub fibre_gate_do: Option<i64>,
    /// `DOLaser`: output port.
    pub fibre_do: Option<i64>,
    /// `DORedLight`: output port.
    pub fibre_red_light_do: Option<i64>,
    /// `LaserMaxPower`, W (`0` on the reference machine).
    pub fibre_max_power_w: Option<f64>,
    /// `CO2LaserControlType`.
    pub co2_control_type: Option<i64>,
    /// `CO2LaserDAPort`.
    pub co2_da_port: Option<i64>,
    /// `CO2DOLaserGate`: output port.
    pub co2_gate_do: Option<i64>,
    /// `CO2DOLaser`: output port.
    pub co2_do: Option<i64>,
    /// `CO2DORedLight`: output port.
    pub co2_red_light_do: Option<i64>,
    /// `doCO2EnableOutput`: output port.
    pub co2_enable_do: Option<i64>,
    /// `LPF.LiftingPlatformType`.
    pub lifting_platform_type: Option<i64>,
}

/// `PGasParam/MGP` and `PManuParam/GC`, `UN`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct GasSettings {
    /// Output ports per gas (`LowAir`, `LowO2`, `LowN2`, `HighAir`, `HighO2`, `HighN2`), in that
    /// order. `0` is not wired.
    pub valve_do: [Option<i64>; 6],
    /// `RatioAir`, `RatioO2`, `RatioH2` (sic): proportional-valve analogue ports.
    pub ratio_da: [Option<i64>; 3],
    /// `DAMaxPressure`: pressure at full analogue output.
    pub da_max_pressure: Option<f64>,
    /// `GC.DefaultGasPressure`.
    pub default_pressure: Option<f64>,
    /// `GC.GasDelay`, ms.
    pub gas_delay_ms: Option<f64>,
    /// `GC.ChangeGasDelay`, ms.
    pub change_gas_delay_ms: Option<f64>,
    /// `UN.GasPressureUnit` (code; whether `0` is MPa or bar is open).
    pub pressure_unit: Option<i64>,
}

/// `PDIParam/DI`: inputs. Ports are the vendor's numbers, `0` is not wired.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct InputSettings {
    /// `EStop`.
    pub estop: Option<i64>,
    /// `WaterWarning` (fibre chiller).
    pub water_warning: Option<i64>,
    /// `WaterWarningType` (level code, meaning open).
    pub water_warning_type: Option<i64>,
    /// `LaserWarning` (fibre source alarm).
    pub laser_warning: Option<i64>,
    /// `LaserWarningType`.
    pub laser_warning_type: Option<i64>,
    /// `CO2WaterWarning` (CO2 chiller).
    pub co2_water_warning: Option<i64>,
    /// `CO2WaterWarningType` (level code, meaning open).
    pub co2_water_warning_type: Option<i64>,
    /// `CO2LaserWarning`.
    pub co2_laser_warning: Option<i64>,
    /// `CO2LaserWarningType`.
    pub co2_laser_warning_type: Option<i64>,
    /// `AlarmDIStr`: named alarm inputs.
    pub alarms: Vec<NamedPort>,
    /// `FunctionDIStr`: named button inputs.
    pub functions: Vec<NamedPort>,
}

/// `PDOParam/DO`: outputs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OutputSettings {
    /// `AlarmSignal`.
    pub alarm_signal: Option<i64>,
    /// `AlarmRing`.
    pub alarm_ring: Option<i64>,
    /// `WaitSignal`.
    pub wait_signal: Option<i64>,
    /// `ManuSignal` (job running).
    pub manu_signal: Option<i64>,
    /// `ManuDoneOutput`.
    pub manu_done: Option<i64>,
    /// `CustomDOStr`: named outputs.
    pub custom: Vec<NamedPort>,
}

/// `PZFParam/ZF` and `PFCParam/FCP`: the height follower as configured. Shown, never changed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FollowerSettings {
    /// `ZFType`.
    pub kind: Option<i64>,
    /// `ZFFollowSpeed`, mm/s.
    pub follow_speed: Option<f64>,
    /// `ZFJogSpeed`, mm/s.
    pub jog_speed: Option<f64>,
    /// `ZFFastJogSpeed`, mm/s.
    pub fast_jog_speed: Option<f64>,
    /// `ZFDockHeight`, mm.
    pub dock_height_mm: Option<f64>,
    /// `ZFUpSpeed`, mm/s.
    pub up_speed: Option<f64>,
    /// `FCP.FrogJumpMinHeight`, mm.
    pub frog_jump_min_height_mm: Option<f64>,
}

/// Jog and manual settings (`PManuParam/MC`, `UN`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JogSettings {
    /// `StepLength`, mm.
    pub step_length_mm: Option<f64>,
    /// `IsStepMove`.
    pub step_move: Option<bool>,
    /// `BoundSpeed`: frame (bounding box) trace speed, mm/s.
    pub bound_speed: Option<f64>,
    /// `ManuAcc`, mm/s².
    pub manu_acc: Option<f64>,
    /// `UN.SpeedUnit` (code).
    pub speed_unit: Option<i64>,
    /// `UN.AccUnit` (code).
    pub acc_unit: Option<i64>,
}

/// Soft limits.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SoftLimits {
    /// `PManuParam/MS.EnableSoftLimit`.
    pub enabled: Option<bool>,
    /// `(slot, SoftLimitMaxLen mm)` per `PMachineAxisConfig_<slot>`.
    pub max_len_mm: Vec<(usize, Option<f64>)>,
}

/// One difference between two profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileChange {
    /// The attribute.
    pub path: AttrPath,
    /// Value on the left, `None` when absent.
    pub left: Option<String>,
    /// Value on the right, `None` when absent.
    pub right: Option<String>,
    /// True for counters, timers and last-used paths that change on every run.
    pub volatile: bool,
}

/// True for attributes that change by themselves while the program runs.
pub fn is_volatile(path: &AttrPath) -> bool {
    let a = path.attr.as_str();
    match path.section.as_str() {
        "PSoftParam" => a.starts_with("Device") || a.ends_with("ManuCount") || matches!(a, "LastLoadFilePath" | "AfterPlanFinishedActionType"),
        "PManuParam" => path.entry == "MSC" && matches!(a, "RecenFileIndex" | "LatestFilePath"),
        _ => false,
    }
}

impl MachineProfile {
    /// Merge documents in order; the first to name an attribute wins.
    pub fn from_docs(sources: &[(&str, &ParamDoc)]) -> MachineProfile {
        let mut p = MachineProfile::default();
        for (label, doc) in sources {
            p.merge(label, doc);
        }
        p
    }

    /// Merge one more document; its values lose to what is already there.
    pub fn merge(&mut self, label: &str, doc: &ParamDoc) {
        let source = self.sources.len();
        self.sources.push(label.to_string());
        for s in &doc.sections {
            let mi = match self.doc.sections.iter().position(|m| m.name == s.name) {
                Some(i) => i,
                None => {
                    self.doc.sections.push(Section { name: s.name.clone(), attrs: s.attrs.clone(), entries: Vec::new() });
                    self.doc.sections.len() - 1
                }
            };
            for (ei, e) in s.entries.iter().enumerate() {
                let occ = occurrence_of(s, ei);
                let Some(merged) = self.doc.sections.get_mut(mi) else { continue };
                let target = match merged.entries.iter().enumerate().filter(|(_, m)| m.name == e.name).nth(occ).map(|(i, _)| i) {
                    Some(i) => i,
                    None => {
                        merged.entries.push(Entry { name: e.name.clone(), attrs: Vec::new() });
                        merged.entries.len() - 1
                    }
                };
                let Some(t) = merged.entries.get_mut(target) else { continue };
                for a in &e.attrs {
                    let path = AttrPath { section: s.name.clone(), entry: e.name.clone(), occurrence: occ, attr: a.name.clone() };
                    match t.attrs.iter().find(|x| x.name == a.name) {
                        Some(x) if !same_value(&x.value(), &a.value()) => {
                            let kept_from = self.origins.get(&path).and_then(|i| self.sources.get(*i)).cloned().unwrap_or_default();
                            self.conflicts.push(Conflict { path, kept: x.value(), kept_from, other: a.value(), other_from: label.to_string() });
                        }
                        Some(_) => {}
                        None => {
                            t.attrs.push(Attr { name: a.name.clone(), raw: a.raw.clone() });
                            self.origins.insert(path, source);
                        }
                    }
                }
            }
        }
    }

    /// The first entry called `entry` in `section`.
    pub fn entry(&self, section: &str, entry: &str) -> Option<&Entry> {
        self.doc.section(section)?.entry(entry)
    }

    fn f(&self, section: &str, entry: &str, attr: &str) -> Option<f64> {
        self.entry(section, entry)?.get_f64(attr)
    }

    fn i(&self, section: &str, entry: &str, attr: &str) -> Option<i64> {
        self.entry(section, entry)?.get_i64(attr)
    }

    fn b(&self, section: &str, entry: &str, attr: &str) -> Option<bool> {
        self.entry(section, entry)?.get_bool(attr)
    }

    /// Sections no typed view reads (layers aside), kept as raw attribute lists.
    pub fn untyped_sections(&self) -> Vec<&Section> {
        self.doc
            .sections
            .iter()
            .filter(|s| !TYPED_SECTIONS.contains(&s.name.as_str()))
            .filter(|s| !s.name.starts_with("PMachineAxisConfig"))
            .filter(|s| LayerKind::from_section_name(&s.name).is_none())
            .collect()
    }

    /// `PAxisParam/A<n>` with the matching `PHomeParam` entry.
    pub fn axes(&self) -> Vec<AxisParam> {
        let Some(sec) = self.doc.section("PAxisParam") else { return Vec::new() };
        sec.entries
            .iter()
            .filter_map(|e| {
                let slot: usize = e.name.strip_prefix('A')?.parse().ok().filter(|n| *n < 64)?;
                let home = self.entry("PHomeParam", &e.name).or_else(|| self.entry("PHomeParam", &format!("HPA{slot}")));
                Some(AxisParam {
                    slot,
                    enable_type: e.get_i64("EnableType"),
                    axis_index: e.get_i64("AxisIndex"),
                    double_driver: e.get_bool("DoubleDriver"),
                    pulse_equivalent: e.get_f64("PulseEquivalent"),
                    max_length_mm: e.get_f64("MaxLength"),
                    limit_switch_type: e.get_i64("LimitSwitchType"),
                    encoder_reverse: e.get_bool("EncoderReverse"),
                    home_direction: home.and_then(|h| h.get_i64("HomeDirection")),
                    home_offset_mm: home.and_then(|h| h.get_f64("HomeOffset")),
                })
            })
            .collect()
    }

    /// `PMachineAxisConfig_<n>` sections, by slot.
    pub fn axis_configs(&self) -> Vec<AxisConfig> {
        let mut out: Vec<AxisConfig> = self
            .doc
            .sections
            .iter()
            .filter_map(|s| {
                let slot: usize = s.name.strip_prefix("PMachineAxisConfig_")?.parse().ok().filter(|n| *n < 64)?;
                let e = s.entries.iter().find(|e| e.name == "MAC" || e.name == format!("MAC_{slot}"))?;
                Some(AxisConfig {
                    slot,
                    soft_limit_max_len_mm: e.get_f64("SoftLimitMaxLen"),
                    write_pulse: e.get_f64("WritePluse"),
                    speed_ratio: e.get_f64("SpeedRatio"),
                    axis_reverse: e.get_bool("AxisReverse"),
                    encoder_reverse: e.get_bool("EncoderReverse"),
                    rotating: e.get_bool("IsRotatingShaft"),
                    negative_limit_input: e.get_i64("NegativeLimitInput"),
                    forward_limit_input: e.get_i64("ForwardLimitInput"),
                    home_fast_speed: e.get_f64("FastSpeed"),
                    home_second_speed: e.get_f64("SecondSpeed"),
                    return_length_mm: e.get_f64("ReturnLength"),
                    acceleration: e.get_f64("Acceleration"),
                })
            })
            .collect();
        out.sort_by_key(|a| a.slot);
        out
    }

    /// Homing.
    pub fn home(&self) -> HomeSettings {
        let s = "PHomeParam";
        HomeSettings {
            use_z_pulse: self.b(s, "HP", "UseZPulse"),
            sample_signal: self.i(s, "HP", "SampleSignal"),
            limit_switch_type: self.i(s, "HP", "LimitSwitchType"),
            fast_speed: self.f(s, "HP", "FastSpeed"),
            slow_speed: self.f(s, "HP", "SlowSpeed"),
            second_pass: self.b(s, "HP", "EnableSecondTimeHome"),
        }
    }

    /// Laser sources.
    pub fn laser(&self) -> LaserSettings {
        let (s, e) = ("PLaserParam", "LGP");
        LaserSettings {
            fibre_type: self.i(s, e, "LaserType"),
            fibre_control_type: self.i(s, e, "LaserControlType"),
            fibre_da_port: self.i(s, e, "LaserDAPort"),
            fibre_gate_do: self.i(s, e, "DOLaserGate"),
            fibre_do: self.i(s, e, "DOLaser"),
            fibre_red_light_do: self.i(s, e, "DORedLight"),
            fibre_max_power_w: self.f(s, e, "LaserMaxPower"),
            co2_control_type: self.i(s, e, "CO2LaserControlType"),
            co2_da_port: self.i(s, e, "CO2LaserDAPort"),
            co2_gate_do: self.i(s, e, "CO2DOLaserGate"),
            co2_do: self.i(s, e, "CO2DOLaser"),
            co2_red_light_do: self.i(s, e, "CO2DORedLight"),
            co2_enable_do: self.i(s, e, "doCO2EnableOutput"),
            lifting_platform_type: self.i(s, "LPF", "LiftingPlatformType"),
        }
    }

    /// Gas.
    pub fn gas(&self) -> GasSettings {
        let (s, e) = ("PGasParam", "MGP");
        GasSettings {
            valve_do: ["LowAir", "LowO2", "LowN2", "HighAir", "HighO2", "HighN2"].map(|a| self.i(s, e, a)),
            ratio_da: ["RatioAir", "RatioO2", "RatioH2"].map(|a| self.i(s, e, a)),
            da_max_pressure: self.f(s, e, "DAMaxPressure"),
            default_pressure: self.f("PManuParam", "GC", "DefaultGasPressure"),
            gas_delay_ms: self.f("PManuParam", "GC", "GasDelay"),
            change_gas_delay_ms: self.f("PManuParam", "GC", "ChangeGasDelay"),
            pressure_unit: self.i("PManuParam", "UN", "GasPressureUnit"),
        }
    }

    /// Inputs.
    pub fn inputs(&self) -> InputSettings {
        let (s, e) = ("PDIParam", "DI");
        let list = |a: &str| self.entry(s, e).and_then(|x| x.get(a)).map(|v| parse_named_ports(&v)).unwrap_or_default();
        InputSettings {
            estop: self.i(s, e, "EStop"),
            water_warning: self.i(s, e, "WaterWarning"),
            water_warning_type: self.i(s, e, "WaterWarningType"),
            laser_warning: self.i(s, e, "LaserWarning"),
            laser_warning_type: self.i(s, e, "LaserWarningType"),
            co2_water_warning: self.i(s, e, "CO2WaterWarning"),
            co2_water_warning_type: self.i(s, e, "CO2WaterWarningType"),
            co2_laser_warning: self.i(s, e, "CO2LaserWarning"),
            co2_laser_warning_type: self.i(s, e, "CO2LaserWarningType"),
            alarms: list("AlarmDIStr"),
            functions: list("FunctionDIStr"),
        }
    }

    /// The CO2 chiller's alarm input when it is wired to a card input (1..=28), for
    /// [`crate::command::SafetyConfig::co2_water_input`]. Which level means "water OK"
    /// (`CO2WaterWarningType`) is not decoded and stays the caller's choice.
    pub fn co2_water_input(&self) -> Option<u32> {
        self.inputs().co2_water_warning.filter(|p| (1..=28).contains(p)).and_then(|p| u32::try_from(p).ok())
    }

    /// Outputs.
    pub fn outputs(&self) -> OutputSettings {
        let (s, e) = ("PDOParam", "DO");
        OutputSettings {
            alarm_signal: self.i(s, e, "AlarmSignal"),
            alarm_ring: self.i(s, e, "AlarmRing"),
            wait_signal: self.i(s, e, "WaitSignal"),
            manu_signal: self.i(s, e, "ManuSignal"),
            manu_done: self.i(s, e, "ManuDoneOutput"),
            custom: self.entry(s, e).and_then(|x| x.get("CustomDOStr")).map(|v| parse_named_ports(&v)).unwrap_or_default(),
        }
    }

    /// Height follower.
    pub fn follower(&self) -> FollowerSettings {
        let (s, e) = ("PZFParam", "ZF");
        FollowerSettings {
            kind: self.i(s, e, "ZFType"),
            follow_speed: self.f(s, e, "ZFFollowSpeed"),
            jog_speed: self.f(s, e, "ZFJogSpeed"),
            fast_jog_speed: self.f(s, e, "ZFFastJogSpeed"),
            dock_height_mm: self.f(s, e, "ZFDockHeight"),
            up_speed: self.f(s, e, "ZFUpSpeed"),
            frog_jump_min_height_mm: self.f("PFCParam", "FCP", "FrogJumpMinHeight"),
        }
    }

    /// Jog and manual.
    pub fn jog(&self) -> JogSettings {
        let (s, e) = ("PManuParam", "MC");
        JogSettings {
            step_length_mm: self.f(s, e, "StepLength"),
            step_move: self.b(s, e, "IsStepMove"),
            bound_speed: self.f(s, e, "BoundSpeed"),
            manu_acc: self.f(s, e, "ManuAcc"),
            speed_unit: self.i(s, "UN", "SpeedUnit"),
            acc_unit: self.i(s, "UN", "AccUnit"),
        }
    }

    /// Soft limits.
    pub fn soft_limits(&self) -> SoftLimits {
        SoftLimits {
            enabled: self.b("PManuParam", "MS", "EnableSoftLimit"),
            max_len_mm: self.axis_configs().iter().map(|a| (a.slot, a.soft_limit_max_len_mm)).collect(),
        }
    }

    fn manu_attr(&self, attr: &str) -> Option<f64> {
        ["MC", "MP"].iter().find_map(|e| self.f("PManuParam", e, attr))
    }

    /// Motion parameters for the planner and the simulator. Values the profile lacks keep the
    /// defaults of [`MachineParams`] and are listed as notes; `K` is not in these files and stays
    /// the default.
    pub fn machine_params(&self) -> (MachineParams, Vec<ReadNote>) {
        let mut p = MachineParams::default();
        let mut notes = Vec::new();
        let mut miss = |attr: &str| notes.push(ReadNote { attr: attr.to_string(), message: "not in the profile; default kept".to_string() });
        let int = |v: f64| (v.fract() == 0.0 && v.abs() < 9.0e15).then_some(v as i64);
        match self.manu_attr("JogFastSpeed") {
            Some(v) => p.jog_fast_speed = v,
            None => miss("JogFastSpeed"),
        }
        match self.manu_attr("JogSlowSpeed") {
            Some(v) => p.jog_slow_speed = v,
            None => miss("JogSlowSpeed"),
        }
        match self.manu_attr("IsFastMode") {
            Some(v) => p.is_fast_mode = v != 0.0,
            None => miss("IsFastMode"),
        }
        match self.manu_attr("XFastMoveAcc") {
            Some(v) => p.x_fast_move_acc = v,
            None => miss("XFastMoveAcc"),
        }
        match self.manu_attr("XFastMoveSpeed") {
            Some(v) => p.x_fast_move_speed = v,
            None => miss("XFastMoveSpeed"),
        }
        match self.f("PFCParam", "FCP", "MaxSpeed") {
            Some(v) => p.fcp_max_speed = v,
            None => miss("MaxSpeed"),
        }
        match self.f("PFCParam", "FCP", "MaxAcc").and_then(int) {
            Some(v) => p.fcp_max_acc = v,
            None => miss("MaxAcc"),
        }
        match self.manu_attr("JogStopDccFactor").and_then(int) {
            Some(v) => p.jog_stop_dcc_factor = v,
            None => miss("JogStopDccFactor"),
        }
        match self.manu_attr("EmptyMoveSpeedFactor") {
            Some(v) => p.empty_move_speed_factor = v,
            None => miss("EmptyMoveSpeedFactor"),
        }
        match self.manu_attr("EmptyMoveAccFactor") {
            Some(v) => p.empty_move_acc_factor = v,
            None => miss("EmptyMoveAccFactor"),
        }
        (p, notes)
    }

    /// The scan (fly-cut) compensation table stored in `PGraphParam/GRP` for a head, when present.
    pub fn scan_fly_table(&self, kind: LayerKind) -> Option<Result<CompensationTable, MlaserError>> {
        let attr = match kind {
            LayerKind::Fibre => "FiberScanFlyCompensateStr",
            LayerKind::Co2 => "CO2ScanFlyCompensateStr",
        };
        let s = self.entry("PGraphParam", "GRP")?.get(attr)?;
        Some(CompensationTable::from_scan_fly_str(&s))
    }

    /// The layer sections as a craft table (see [`LayerTable::to_craft_table`]).
    pub fn craft_table(&self) -> (CraftTable, Vec<ReadNote>) {
        LayerTable { doc: self.doc.clone() }.to_craft_table()
    }

    /// True if the profile has any layer section.
    pub fn has_layers(&self) -> bool {
        self.doc.sections.iter().any(|s| LayerKind::from_section_name(&s.name).is_some())
    }

    /// Every attribute with its path, in document order.
    pub fn attributes(&self) -> Vec<(AttrPath, String)> {
        let mut out = Vec::new();
        for s in &self.doc.sections {
            for (ei, e) in s.entries.iter().enumerate() {
                let occ = occurrence_of(s, ei);
                for a in &e.attrs {
                    out.push((AttrPath { section: s.name.clone(), entry: e.name.clone(), occurrence: occ, attr: a.name.clone() }, a.value()));
                }
            }
        }
        out
    }
}

/// Differences between two profiles, compared as numbers where both sides are numbers. Volatile
/// attributes are included and flagged.
pub fn diff(left: &MachineProfile, right: &MachineProfile) -> Vec<ProfileChange> {
    let lookup = |p: &MachineProfile, path: &AttrPath| -> Option<String> {
        let s = p.doc.section(&path.section)?;
        nth_entry(s, &path.entry, path.occurrence)?.get(&path.attr)
    };
    let mut out = Vec::new();
    for (path, lv) in left.attributes() {
        match lookup(right, &path) {
            Some(rv) if same_value(&lv, &rv) => {}
            rv => out.push(ProfileChange { volatile: is_volatile(&path), left: Some(lv), right: rv, path }),
        }
    }
    for (path, rv) in right.attributes() {
        if lookup(left, &path).is_none() {
            out.push(ProfileChange { volatile: is_volatile(&path), left: None, right: Some(rv), path });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HARD: &str = "<ParameterRoot>\r\n\
<PAxisParam>\r\n\
<A0 EnableType=\"0\" AxisIndex=\"0\" DoubleDriver=\"0\" PulseEquivalent=\"1\" MaxLength=\"1200\" LimitSwitchType=\"0\" EncoderReverse=\"0\"/>\r\n\
<A1 EnableType=\"0\" AxisIndex=\"1\" DoubleDriver=\"1\" PulseEquivalent=\"1\" MaxLength=\"800\" LimitSwitchType=\"0\" EncoderReverse=\"1\"/>\r\n\
<AX InterpolationCycle=\"250\"/>\r\n\
</PAxisParam>\r\n\
<PHomeParam>\r\n\
<HP UseZPulse=\"0\" SampleSignal=\"1\" LimitSwitchType=\"0\" FastSpeed=\"40\" SlowSpeed=\"8\" EnableSecondTimeHome=\"1\"/>\r\n\
<A0 HomeDirection=\"1\" HomeOffset=\"5\"/>\r\n\
</PHomeParam>\r\n\
<PLaserParam>\r\n\
<LGP LaserType=\"0\" LaserControlType=\"3\" LaserDAPort=\"1\" DOLaserGate=\"4\" DOLaser=\"0\" DORedLight=\"7\" LaserMaxPower=\"0\" CO2LaserControlType=\"2\" CO2LaserDAPort=\"2\" CO2DOLaserGate=\"0\" CO2DOLaser=\"8\" CO2DORedLight=\"0\" doCO2EnableOutput=\"0\"/>\r\n\
<LPF LiftingPlatformType=\"1\"/>\r\n\
</PLaserParam>\r\n\
<PGasParam>\r\n\
<MGP LowAir=\"0\" LowO2=\"6\" LowN2=\"0\" HighAir=\"2\" HighO2=\"0\" HighN2=\"3\" RatioAir=\"0\" RatioO2=\"1\" RatioH2=\"0\" DAMaxPressure=\"10\"/>\r\n\
</PGasParam>\r\n\
<PDOParam>\r\n\
<DO AlarmSignal=\"2\" AlarmRing=\"0\" WaitSignal=\"0\" ManuSignal=\"0\" CustomDOStr=\"Fan#3#-1,Lamp#0#0\"/>\r\n\
</PDOParam>\r\n\
<PDIParam>\r\n\
<DI EStop=\"0\" WaterWarning=\"0\" WaterWarningType=\"0\" LaserWarning=\"2\" LaserWarningType=\"0\" AlarmDIStr=\"Lid#5#-1#0\" FunctionDIStr=\"Start#0#0,Stop#9#-1\" CO2WaterWarning=\"12\" CO2WaterWarningType=\"1\" CO2LaserWarning=\"0\" CO2LaserWarningType=\"0\"/>\r\n\
<SP EnableEncoderVel=\"0\"/>\r\n\
<DO EnableBackRollSheet=\"0\"/>\r\n\
</PDIParam>\r\n\
<PFCParam>\r\n\
<FCP FrogJumpMinHeight=\"8\" MaxSpeed=\"2500\" MaxAcc=\"15000\"/>\r\n\
</PFCParam>\r\n\
<PSoftParam>\r\n\
<SOP RemoteType=\"0\" HardwareType=\"1\"/>\r\n\
</PSoftParam>\r\n\
<PMachineAxisConfig_0>\r\n\
<MAC SoftLimitMaxLen=\"1210\" WritePluse=\"8000\" AxisReverse=\"1\" SpeedRatio=\"30\" IsRotatingShaft=\"0\" NegativeLimitInput=\"6\" ForwardLimitInput=\"5\" FastSpeed=\"60\" SecondSpeed=\"15\" ReturnLength=\"12\" Acceleration=\"18000\"/>\r\n\
</PMachineAxisConfig_0>\r\n\
<PMachineAxisConfig_1>\r\n\
<MAC_1 SoftLimitMaxLen=\"810\" WritePluse=\"8000\" AxisReverse=\"0\" SpeedRatio=\"30\"/>\r\n\
</PMachineAxisConfig_1>\r\n\
</ParameterRoot>\r\n";

    const MANU: &str = "<ParameterRoot>\r\n\
<PManuParam>\r\n\
<MP JogStopDccFactor=\"2\" EmptyMoveSpeedFactor=\"1.2\" EmptyMoveAccFactor=\"1.25\"/>\r\n\
<MC IsStepMove=\"0\" IsFastMode=\"0\" StepLength=\"2\" JogFastSpeed=\"150\" JogSlowSpeed=\"30\" BoundSpeed=\"400\" XFastMoveSpeed=\"450\" XFastMoveAcc=\"4000\" ManuAcc=\"3000\"/>\r\n\
<GC DefaultGasPressure=\"0.80000000000000004\" GasDelay=\"150\" DirectGasDelay=\"100\" ChangeGasDelay=\"120\"/>\r\n\
<MS AutoClearManuLine=\"1\" EnableSoftLimit=\"1\"/>\r\n\
<UN SpeedUnit=\"1\" AccUnit=\"1\" GasPressureUnit=\"0\"/>\r\n\
<MSC RecenFileIndex=\"0\" LatestFilePath=\"L&quot;&quot;\"/>\r\n\
</PManuParam>\r\n\
<PSoftParam>\r\n\
<SOP LastLoadFilePath=\"demo.chf\" ManuCount=\"4\" HardwareType=\"2\"/>\r\n\
</PSoftParam>\r\n\
<PZFParam>\r\n\
<ZF ZFType=\"1\" ZFFollowSpeed=\"180\" ZFJogSpeed=\"10\" ZFFastJogSpeed=\"40\" ZFDockHeight=\"15\" ZFUpSpeed=\"250\"/>\r\n\
</PZFParam>\r\n\
<PGraphParam>\r\n\
<GRP FiberScanFlyCompensateStr=\"100#0.25,200#0.5\" CO2ScanFlyCompensateStr=\"100#x\"/>\r\n\
</PGraphParam>\r\n\
<PNestParam>\r\n\
<NP Gap=\"2\"/>\r\n\
</PNestParam>\r\n\
</ParameterRoot>\r\n";

    fn profile() -> MachineProfile {
        let hard = ParamDoc::parse(HARD.as_bytes()).unwrap();
        let manu = ParamDoc::parse(MANU.as_bytes()).unwrap();
        MachineProfile::from_docs(&[("hard", &hard), ("manu", &manu)])
    }

    #[test]
    fn merges_split_entries_and_records_conflicts() {
        let p = profile();
        assert_eq!(p.sources, ["hard", "manu"]);
        let sop = p.entry("PSoftParam", "SOP").unwrap();
        assert_eq!(sop.get("RemoteType").as_deref(), Some("0"));
        assert_eq!(sop.get("ManuCount").as_deref(), Some("4"));
        assert_eq!(p.conflicts.len(), 1);
        let c = &p.conflicts[0];
        assert_eq!(c.path.to_string(), "PSoftParam/SOP.HardwareType");
        assert_eq!((c.kept.as_str(), c.kept_from.as_str(), c.other.as_str(), c.other_from.as_str()), ("1", "hard", "2", "manu"));
        // The same entry name in two sections stays apart.
        assert!(p.entry("PDOParam", "DO").unwrap().has("AlarmSignal"));
        assert!(!p.entry("PDIParam", "DO").unwrap().has("AlarmSignal"));
        let untyped: Vec<&str> = p.untyped_sections().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(untyped, ["PSoftParam", "PNestParam"]);
    }

    #[test]
    fn typed_views() {
        let p = profile();
        let axes = p.axes();
        assert_eq!(axes.len(), 2);
        assert_eq!(axes[0].max_length_mm, Some(1200.0));
        assert_eq!(axes[0].home_direction, Some(1));
        assert_eq!(axes[0].home_offset_mm, Some(5.0));
        assert_eq!(axes[1].double_driver, Some(true));
        assert_eq!(axes[1].home_offset_mm, None);
        let cfg = p.axis_configs();
        assert_eq!(cfg.iter().map(|c| c.slot).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(cfg[0].acceleration, Some(18000.0));
        assert_eq!(cfg[1].soft_limit_max_len_mm, Some(810.0));
        assert_eq!(p.soft_limits(), SoftLimits { enabled: Some(true), max_len_mm: vec![(0, Some(1210.0)), (1, Some(810.0))] });
        assert_eq!(p.home().second_pass, Some(true));
        let l = p.laser();
        assert_eq!((l.fibre_gate_do, l.co2_do, l.lifting_platform_type), (Some(4), Some(8), Some(1)));
        let g = p.gas();
        assert_eq!(g.valve_do, [Some(0), Some(6), Some(0), Some(2), Some(0), Some(3)]);
        assert_eq!(g.default_pressure, Some(0.8));
        assert_eq!(g.pressure_unit, Some(0));
        let i = p.inputs();
        assert_eq!(i.co2_water_warning, Some(12));
        assert_eq!(i.co2_water_warning_type, Some(1));
        assert_eq!(i.alarms, vec![NamedPort { name: "Lid".into(), port: Some(5), rest: vec!["-1".into(), "0".into()] }]);
        assert_eq!(i.functions[1].port, Some(9));
        assert_eq!(p.co2_water_input(), Some(12));
        let o = p.outputs();
        assert_eq!(o.alarm_signal, Some(2));
        assert_eq!(o.custom.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Fan", "Lamp"]);
        let f = p.follower();
        assert_eq!((f.follow_speed, f.dock_height_mm, f.frog_jump_min_height_mm), (Some(180.0), Some(15.0), Some(8.0)));
        let j = p.jog();
        assert_eq!((j.step_length_mm, j.bound_speed, j.speed_unit), (Some(2.0), Some(400.0), Some(1)));
        let fibre = p.scan_fly_table(LayerKind::Fibre).unwrap().unwrap();
        assert_eq!(fibre.offset_at(150.0), Some(0.375));
        assert!(p.scan_fly_table(LayerKind::Co2).unwrap().is_err());
        assert!(!p.has_layers());
    }

    #[test]
    fn machine_params_from_profile() {
        let (m, notes) = profile().machine_params();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(m.k, 1000);
        assert_eq!((m.jog_fast_speed, m.jog_slow_speed, m.is_fast_mode), (150.0, 30.0, false));
        assert_eq!((m.x_fast_move_speed, m.x_fast_move_acc), (450.0, 4000.0));
        assert_eq!((m.fcp_max_speed, m.fcp_max_acc), (2500.0, 15000));
        assert_eq!((m.jog_stop_dcc_factor, m.empty_move_speed_factor, m.empty_move_acc_factor), (2, 1.2, 1.25));
        let (d, notes) = MachineProfile::default().machine_params();
        assert_eq!(d, MachineParams::default());
        assert_eq!(notes.len(), 10);
    }

    #[test]
    fn co2_water_input_needs_a_card_input() {
        for (v, want) in [("0", None), ("29", None), ("-3", None), ("1.5", None), ("28", Some(28))] {
            let doc =
                ParamDoc::parse(format!("<ParameterRoot><PDIParam><DI CO2WaterWarning=\"{v}\"/></PDIParam></ParameterRoot>").as_bytes()).unwrap();
            assert_eq!(MachineProfile::from_docs(&[("x", &doc)]).co2_water_input(), want, "{v}");
        }
    }

    #[test]
    fn diff_compares_numbers_and_flags_volatile_keys() {
        let a = profile();
        let mut manu = ParamDoc::parse(MANU.as_bytes()).unwrap();
        let hard = ParamDoc::parse(HARD.as_bytes()).unwrap();
        let mc = manu.section_mut("PManuParam").unwrap().entry_mut("MC").unwrap();
        mc.set("JogFastSpeed", "150.0");
        mc.set("BoundSpeed", "420");
        mc.remove("ManuAcc");
        let sop = manu.section_mut("PSoftParam").unwrap().entry_mut("SOP").unwrap();
        sop.set("ManuCount", "5");
        manu.section_mut("PNestParam").unwrap().entries.push(Entry { name: "NP".into(), attrs: vec![Attr { name: "Gap".into(), raw: "3".into() }] });
        let b = MachineProfile::from_docs(&[("hard", &hard), ("manu", &manu)]);
        let d = diff(&a, &b);
        let show: Vec<String> = d.iter().map(|c| format!("{} {:?} {:?} {}", c.path, c.left, c.right, c.volatile)).collect();
        assert_eq!(
            show,
            [
                "PSoftParam/SOP.ManuCount Some(\"4\") Some(\"5\") true",
                "PManuParam/MC.BoundSpeed Some(\"400\") Some(\"420\") false",
                "PManuParam/MC.ManuAcc Some(\"3000\") None false",
                "PNestParam/NP[1].Gap None Some(\"3\") false",
            ]
        );
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn layers_in_a_profile_become_a_craft_table() {
        let fibre =
            crate::recipe::write_recipe(&crate::craft::Craft::Fibre(crate::craft::FibreCraft { name: "demo".into(), ..Default::default() })).unwrap();
        let doc = ParamDoc::parse(&fibre).unwrap();
        let p = MachineProfile::from_docs(&[("layers", &doc)]);
        assert!(p.has_layers());
        assert!(p.untyped_sections().is_empty());
        let (t, _) = p.craft_table();
        assert_eq!(t.fibre[10].name, "demo");
    }

    #[test]
    fn hostile_profiles_do_not_panic() {
        for src in [
            "<ParameterRoot><PAxisParam><A99999999999999999999 MaxLength=\"nan\"/><A/><A-1/></PAxisParam></ParameterRoot>",
            "<ParameterRoot><PMachineAxisConfig_9999><MAC SoftLimitMaxLen=\"1e400\"/></PMachineAxisConfig_9999><PMachineAxisConfig_x/></ParameterRoot>",
            "<ParameterRoot><PDIParam><DI AlarmDIStr=\"###,,#,a#1e300#\" FunctionDIStr=\"\" CO2WaterWarning=\"99999999999999999999\"/></PDIParam></ParameterRoot>",
            "<ParameterRoot><PFCParam><FCP MaxAcc=\"1.5\" MaxSpeed=\"-inf\"/></PFCParam><PGraphParam><GRP FiberScanFlyCompensateStr=\"#,#,#\"/></PGraphParam></ParameterRoot>",
        ] {
            let doc = ParamDoc::parse(src.as_bytes()).unwrap();
            let p = MachineProfile::from_docs(&[("a", &doc), ("b", &doc)]);
            assert!(p.conflicts.is_empty());
            let _ = (p.axes(), p.axis_configs(), p.inputs(), p.outputs(), p.gas(), p.laser(), p.follower(), p.jog(), p.home(), p.soft_limits());
            let _ =
                (p.machine_params(), p.co2_water_input(), p.scan_fly_table(LayerKind::Fibre), p.craft_table(), diff(&p, &MachineProfile::default()));
        }
    }
}
