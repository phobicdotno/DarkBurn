//! Layer / craft parameters of the Gweike M3 Ultra (MCC-04): fibre cutting with assist gas, pierce
//! stages and follower height, and the CO2 variant.
//!
//! The model follows the owner's analysis of the per-layer parameter file
//! (`02-layer-params.md`). A `.chf` only carries a layer *index* ([`crate::chf::Contour::layer`]);
//! the parameters live outside it, one record per layer, 11 layers per head. This module is the
//! data model, with validation. It does not drive the machine: the pierce sequence, the height
//! follower and the gas valves are run by the card and the follower unit, which stay in charge.
//!
//! Units: speeds mm/s, heights and focus mm, power and duty percent, frequency Hz, times ms, gas
//! pressure bar (the vendor descriptor says volts; the owner's analysis infers bar through a
//! calibration table, unconfirmed).
//!
//! Gas codes (inferred, high confidence): 0 air, 1 O2, 2 N2, 3 high-pressure air, 4 high-pressure
//! O2, 5 high-pressure N2. Process types (high confidence for 0 to 4): 0 direct cut, 1 fixed-height
//! cut, 2 one pierce stage, 3 two, 4 three.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Assist gas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GasType {
    /// Low-pressure air (code 0).
    #[default]
    Air,
    /// Low-pressure oxygen (code 1).
    O2,
    /// Low-pressure nitrogen (code 2).
    N2,
    /// High-pressure air (code 3).
    HighAir,
    /// High-pressure oxygen (code 4).
    HighO2,
    /// High-pressure nitrogen (code 5).
    HighN2,
}

impl GasType {
    /// The vendor's integer code.
    pub fn code(self) -> u8 {
        match self {
            GasType::Air => 0,
            GasType::O2 => 1,
            GasType::N2 => 2,
            GasType::HighAir => 3,
            GasType::HighO2 => 4,
            GasType::HighN2 => 5,
        }
    }

    /// From the vendor's integer code.
    pub fn from_code(code: u8) -> Option<GasType> {
        Some(match code {
            0 => GasType::Air,
            1 => GasType::O2,
            2 => GasType::N2,
            3 => GasType::HighAir,
            4 => GasType::HighO2,
            5 => GasType::HighN2,
            _ => return None,
        })
    }
}

/// How a contour is started and cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProcessType {
    /// Direct cut, no pierce (code 0).
    #[default]
    DirectCut,
    /// Fixed-height cut at the fixed-height position, no pierce (code 1).
    FixedHeight,
    /// One pierce stage (code 2).
    Pierce1,
    /// Two pierce stages (code 3).
    Pierce2,
    /// Three pierce stages (code 4).
    Pierce3,
}

impl ProcessType {
    /// The vendor's `ManuType` code.
    pub fn code(self) -> u8 {
        match self {
            ProcessType::DirectCut => 0,
            ProcessType::FixedHeight => 1,
            ProcessType::Pierce1 => 2,
            ProcessType::Pierce2 => 3,
            ProcessType::Pierce3 => 4,
        }
    }

    /// From the code. Codes above 4 are not established.
    pub fn from_code(code: u8) -> Option<ProcessType> {
        Some(match code {
            0 => ProcessType::DirectCut,
            1 => ProcessType::FixedHeight,
            2 => ProcessType::Pierce1,
            3 => ProcessType::Pierce2,
            4 => ProcessType::Pierce3,
            _ => return None,
        })
    }

    /// Pierce stages this process type runs.
    pub fn stages(self) -> usize {
        match self {
            ProcessType::DirectCut | ProcessType::FixedHeight => 0,
            ProcessType::Pierce1 => 1,
            ProcessType::Pierce2 => 2,
            ProcessType::Pierce3 => 3,
        }
    }
}

/// One node of a speed-dependent power or frequency curve: at `speed_pct` percent of the cut speed
/// the output is `output_pct` percent (inferred).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurveNode {
    /// Speed, percent of the cut speed.
    pub speed_pct: f64,
    /// Output, percent.
    pub output_pct: f64,
}

/// Parse the vendor's flat `x,y,x,y,...` curve string. An empty string is an empty curve.
pub fn parse_curve_nodes(s: &str) -> Result<Vec<CurveNode>, CraftError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let nums: Result<Vec<f64>, _> = s.split(',').map(|t| t.trim().parse::<f64>()).collect();
    let nums = nums.map_err(|_| CraftError::BadCurve(s.to_string()))?;
    if nums.len() % 2 != 0 || nums.iter().any(|v| !v.is_finite()) {
        return Err(CraftError::BadCurve(s.to_string()));
    }
    Ok(nums.chunks(2).map(|c| CurveNode { speed_pct: c[0], output_pct: c[1] }).collect())
}

/// Format curve nodes as the vendor's flat string.
pub fn format_curve_nodes(nodes: &[CurveNode]) -> String {
    nodes.iter().map(|n| format!("{},{}", n.speed_pct, n.output_pct)).collect::<Vec<_>>().join(",")
}

/// Errors of this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CraftError {
    /// A curve string is not an even list of numbers.
    BadCurve(String),
}

impl fmt::Display for CraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CraftError::BadCurve(s) => write!(f, "curve {s:?} is not a list of x,y pairs"),
        }
    }
}

impl std::error::Error for CraftError {}

/// Focus ramp during a pierce stage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusRamp {
    /// Focus at the end of the ramp, mm.
    pub end_focus_mm: f64,
    /// Ramp time, ms.
    pub time_ms: f64,
}

/// One pierce stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PierceStage {
    /// Nozzle height during the stage, mm.
    pub height_mm: f64,
    /// Focus position, mm.
    pub focus_mm: f64,
    /// Optional focus ramp.
    pub focus_ramp: Option<FocusRamp>,
    /// Duty, percent.
    pub power_pct: f64,
    /// Pulse frequency, Hz.
    pub freq_hz: f64,
    /// Peak current, percent of the laser's peak power.
    pub peak_current_pct: f64,
    /// Gas during the stage (may differ from the cut gas).
    pub gas: GasType,
    /// Gas pressure, bar.
    pub gas_pressure_bar: f64,
    /// Stage duration with the laser on, ms.
    pub duration_ms: f64,
    /// Gradual pierce: descend over this many ms (the editor keeps it equal to the duration).
    pub gradual_time_ms: Option<f64>,
    /// Dwell with the laser on at the end, ms.
    pub before_laser_off_delay_ms: f64,
    /// Gas blow after laser off, ms.
    pub after_laser_off_delay_ms: f64,
}

impl Default for PierceStage {
    fn default() -> PierceStage {
        PierceStage {
            height_mm: 5.0,
            focus_mm: 0.0,
            focus_ramp: None,
            power_pct: 50.0,
            freq_hz: 500.0,
            peak_current_pct: 100.0,
            gas: GasType::Air,
            gas_pressure_bar: 5.0,
            duration_ms: 1000.0,
            gradual_time_ms: None,
            before_laser_off_delay_ms: 0.0,
            after_laser_off_delay_ms: 0.0,
        }
    }
}

/// Cutting phase of a fibre craft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutSettings {
    /// Nozzle height while cutting, mm (follower set point).
    pub height_mm: f64,
    /// Focus position, mm. The auto-focus head is disabled on the owner's machine, so this is 0
    /// there and focus is set by hand.
    pub focus_mm: f64,
    /// Speed, mm/s.
    pub speed_mm_s: f64,
    /// Duty, percent.
    pub power_pct: f64,
    /// Pulse frequency, Hz.
    pub freq_hz: f64,
    /// Peak current, percent.
    pub peak_current_pct: f64,
    /// Assist gas.
    pub gas: GasType,
    /// Gas pressure, bar.
    pub gas_pressure_bar: f64,
    /// Dwell after the laser is on, before motion, ms.
    pub laser_on_delay_ms: f64,
    /// Dwell at the contour end before laser off, ms.
    pub laser_off_before_delay_ms: f64,
    /// Dwell after laser off, ms.
    pub laser_off_after_delay_ms: f64,
    /// Lift height between contours, mm.
    pub up_height_mm: f64,
}

impl Default for CutSettings {
    fn default() -> CutSettings {
        CutSettings {
            height_mm: 1.0,
            focus_mm: 0.0,
            speed_mm_s: 100.0,
            power_pct: 100.0,
            freq_hz: 5000.0,
            peak_current_pct: 100.0,
            gas: GasType::Air,
            gas_pressure_bar: 5.0,
            laser_on_delay_ms: 0.0,
            laser_off_before_delay_ms: 0.0,
            laser_off_after_delay_ms: 0.0,
            up_height_mm: 20.0,
        }
    }
}

/// Follower and lift behaviour of a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowerSettings {
    /// Capacitive height following on. (The vendor flag is the negation, `NoFollow`.)
    pub follow: bool,
    /// Position for fixed-height cutting, mm (`AdvFixHeightCutPos`).
    pub fixed_height_cut_pos_mm: f64,
    /// Do not lift between contours that start closer than the global short-move distance.
    pub short_distance_no_lift: bool,
    /// Pierce all start points of the layer first.
    pub pre_drill: bool,
    /// Do not lift between pre-pierces.
    pub pre_drill_no_lift: bool,
    /// Pierce again when the cut starts after a pre-pierce.
    pub pierce_again_before_cut: bool,
    /// Film-removal pass (cut height must not exceed the film cut height).
    pub with_film: bool,
}

impl Default for FollowerSettings {
    fn default() -> FollowerSettings {
        FollowerSettings {
            follow: true,
            fixed_height_cut_pos_mm: 20.0,
            short_distance_no_lift: true,
            pre_drill: false,
            pre_drill_no_lift: false,
            pierce_again_before_cut: false,
            with_film: false,
        }
    }
}

/// Slow start at the beginning of each cut.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowStart {
    /// Length of the slow section, mm.
    pub length_mm: f64,
    /// Speed over that section, mm/s.
    pub speed_mm_s: f64,
}

/// Smooth ("imperceptible") pierce used instead of the staged pierce for thin material.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmoothPierce {
    /// Height, mm.
    pub height_mm: f64,
    /// Time, ms.
    pub time_ms: f64,
    /// Focus, mm.
    pub focus_mm: f64,
    /// Duty, percent.
    pub power_pct: f64,
    /// Frequency, Hz.
    pub freq_hz: f64,
    /// Peak current, percent.
    pub peak_current_pct: f64,
}

/// Gas valve behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GasFlags {
    /// Keep the gas on during short moves.
    pub short_dist_keep_on: bool,
    /// Never close the gas valve during the job.
    pub keep_on_during_job: bool,
}

/// Fibre (1200 W) craft for one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FibreCraft {
    /// Name, usually the technology-library preset it came from.
    pub name: String,
    /// Free notes.
    pub note: String,
    /// Nozzle code such as `1.5S` or `4.0D`, kept as text. The vendor stores it only in the preset
    /// name.
    pub nozzle: Option<String>,
    /// The layer is skipped (drawn only).
    pub no_manu: bool,
    /// Process type.
    pub process: ProcessType,
    /// Cutting phase.
    pub cut: CutSettings,
    /// Pierce stages in order; at least [`ProcessType::stages`] are used, up to five are stored.
    pub pierce: Vec<PierceStage>,
    /// Follower behaviour.
    pub follower: FollowerSettings,
    /// Gas valve behaviour.
    pub gas_flags: GasFlags,
    /// Power curve over speed, used when `dynamic_power` is on.
    pub power_curve: Vec<CurveNode>,
    /// Frequency curve over speed, used when `dynamic_freq` is on.
    pub freq_curve: Vec<CurveNode>,
    /// Adjust power with speed.
    pub dynamic_power: bool,
    /// Adjust frequency with speed.
    pub dynamic_freq: bool,
    /// Slow start.
    pub slow_start: Option<SlowStart>,
    /// Smooth pierce.
    pub smooth_pierce: Option<SmoothPierce>,
}

impl Default for FibreCraft {
    fn default() -> FibreCraft {
        FibreCraft {
            name: String::new(),
            note: String::new(),
            nozzle: None,
            no_manu: false,
            process: ProcessType::DirectCut,
            cut: CutSettings::default(),
            pierce: Vec::new(),
            follower: FollowerSettings::default(),
            gas_flags: GasFlags::default(),
            power_curve: Vec::new(),
            freq_curve: Vec::new(),
            dynamic_power: false,
            dynamic_freq: false,
            slow_start: None,
            smooth_pierce: None,
        }
    }
}

/// CO2 craft for one layer. The CO2 head has no capacitive follower and no pierce stages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Co2Craft {
    /// Name.
    pub name: String,
    /// Notes.
    pub note: String,
    /// Speed, mm/s.
    pub speed_mm_s: f64,
    /// Duty, percent.
    pub duty_pct: f64,
    /// PWM frequency, Hz.
    pub freq_hz: f64,
    /// Assist gas.
    pub gas: GasType,
    /// Gas pressure, bar.
    pub gas_pressure_bar: f64,
    /// Delay after the laser is on, ms.
    pub laser_on_delay_ms: f64,
    /// Fixed head position, mm (`AdvFixHeightCutPos`).
    pub fixed_height_cut_pos_mm: f64,
    /// Gas valve behaviour.
    pub gas_flags: GasFlags,
    /// Power curve.
    pub power_curve: Vec<CurveNode>,
    /// Frequency curve.
    pub freq_curve: Vec<CurveNode>,
    /// Adjust power with speed.
    pub dynamic_power: bool,
    /// Adjust frequency with speed.
    pub dynamic_freq: bool,
    /// Slow start.
    pub slow_start: Option<SlowStart>,
}

impl Default for Co2Craft {
    fn default() -> Co2Craft {
        Co2Craft {
            name: String::new(),
            note: String::new(),
            speed_mm_s: 100.0,
            duty_pct: 100.0,
            freq_hz: 5000.0,
            gas: GasType::Air,
            gas_pressure_bar: 5.0,
            laser_on_delay_ms: 0.0,
            fixed_height_cut_pos_mm: 20.0,
            gas_flags: GasFlags::default(),
            power_curve: Vec::new(),
            freq_curve: Vec::new(),
            dynamic_power: false,
            dynamic_freq: false,
            slow_start: None,
        }
    }
}

/// A craft for either head.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "head", rename_all = "snake_case")]
pub enum Craft {
    /// Fibre head.
    Fibre(FibreCraft),
    /// CO2 head.
    Co2(Co2Craft),
}

/// Number of layers per head in the vendor file.
pub const LAYERS_PER_HEAD: usize = 11;

/// All layers of both heads. Index = the layer index a `.chf` contour carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CraftTable {
    /// Fibre layers.
    pub fibre: Vec<FibreCraft>,
    /// CO2 layers.
    pub co2: Vec<Co2Craft>,
}

impl Default for CraftTable {
    fn default() -> CraftTable {
        CraftTable { fibre: vec![FibreCraft::default(); LAYERS_PER_HEAD], co2: vec![Co2Craft::default(); LAYERS_PER_HEAD] }
    }
}

impl CraftTable {
    /// The craft of a layer on a head.
    pub fn fibre_layer(&self, layer: i32) -> Option<&FibreCraft> {
        usize::try_from(layer).ok().and_then(|i| self.fibre.get(i))
    }

    /// The CO2 craft of a layer.
    pub fn co2_layer(&self, layer: i32) -> Option<&Co2Craft> {
        usize::try_from(layer).ok().and_then(|i| self.co2.get(i))
    }

    /// Validate every layer. Issues carry the head and layer index in their path.
    pub fn validate(&self) -> Vec<CraftIssue> {
        let mut out = Vec::new();
        for (i, c) in self.fibre.iter().enumerate() {
            out.extend(c.validate().into_iter().map(|mut x| {
                x.path = format!("fibre[{i}].{}", x.path);
                x
            }));
        }
        for (i, c) in self.co2.iter().enumerate() {
            out.extend(c.validate().into_iter().map(|mut x| {
                x.path = format!("co2[{i}].{}", x.path);
                x
            }));
        }
        if self.fibre.len() > LAYERS_PER_HEAD || self.co2.len() > LAYERS_PER_HEAD {
            out.push(CraftIssue::error("table", format!("more than {LAYERS_PER_HEAD} layers per head")));
        }
        out
    }
}

/// Severity of a validation finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// The vendor software would refuse or the value is out of range.
    Error,
    /// Suspicious; legacy rule or unusual value.
    Warning,
}

/// A validation finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftIssue {
    /// Severity.
    pub severity: Severity,
    /// Field path.
    pub path: String,
    /// Message.
    pub message: String,
}

impl CraftIssue {
    fn error(path: &str, message: impl Into<String>) -> CraftIssue {
        CraftIssue { severity: Severity::Error, path: path.to_string(), message: message.into() }
    }
    fn warning(path: &str, message: impl Into<String>) -> CraftIssue {
        CraftIssue { severity: Severity::Warning, path: path.to_string(), message: message.into() }
    }
}

fn check_range(out: &mut Vec<CraftIssue>, path: &str, v: f64, lo: f64, hi: f64, what: &str) {
    if !v.is_finite() || v < lo || v > hi {
        out.push(CraftIssue::error(path, format!("{what} {v} outside {lo}..={hi}")));
    }
}

fn check_nonneg(out: &mut Vec<CraftIssue>, path: &str, v: f64, what: &str) {
    if !v.is_finite() || v < 0.0 {
        out.push(CraftIssue::error(path, format!("{what} {v} must be zero or more")));
    }
}

/// Speed ceiling of the owner's machine (`FCP.MaxSpeed`), mm/s.
pub const MAX_SPEED_MM_S: f64 = 3000.0;
/// Pulse frequency range accepted by the vendor software, Hz (`ec19`).
pub const FREQ_RANGE_HZ: (f64, f64) = (1.0, 50000.0);
/// Highest gas pressure the proportional valve accepts, bar (`DAMaxPressure`).
pub const MAX_PRESSURE_BAR: f64 = 10.0;

fn check_curve(out: &mut Vec<CraftIssue>, path: &str, nodes: &[CurveNode], enabled: bool) {
    if nodes.is_empty() {
        if enabled {
            out.push(CraftIssue::error(path, "curve enabled but empty"));
        }
        return;
    }
    for (i, n) in nodes.iter().enumerate() {
        check_range(out, &format!("{path}[{i}]"), n.speed_pct, 0.0, 100.0, "curve speed");
        check_range(out, &format!("{path}[{i}]"), n.output_pct, 0.0, 100.0, "curve output");
    }
    if nodes[0].speed_pct != 0.0 {
        out.push(CraftIssue::warning(path, "first node should start at speed 0 (all vendor presets do)"));
    }
    let last = nodes[nodes.len() - 1];
    if last.speed_pct != 100.0 || last.output_pct != 100.0 {
        out.push(CraftIssue::warning(path, "last node should be 100,100 (all vendor presets do)"));
    }
    if nodes.windows(2).any(|w| w[1].speed_pct < w[0].speed_pct) {
        out.push(CraftIssue::error(path, "curve speeds must not decrease"));
    }
}

fn check_slow_start(out: &mut Vec<CraftIssue>, s: &Option<SlowStart>) {
    if let Some(s) = s {
        check_nonneg(out, "slow_start.length_mm", s.length_mm, "slow-start length");
        if !s.speed_mm_s.is_finite() || s.speed_mm_s <= 0.0 || s.speed_mm_s > MAX_SPEED_MM_S {
            out.push(CraftIssue::error("slow_start.speed_mm_s", format!("slow-start speed {} outside (0, {MAX_SPEED_MM_S}]", s.speed_mm_s)));
        }
    }
}

impl FibreCraft {
    /// Check ranges and the rules between fields.
    pub fn validate(&self) -> Vec<CraftIssue> {
        let mut out = Vec::new();
        let c = &self.cut;
        if !c.speed_mm_s.is_finite() || c.speed_mm_s <= 0.0 || c.speed_mm_s > MAX_SPEED_MM_S {
            out.push(CraftIssue::error("cut.speed_mm_s", format!("speed {} outside (0, {MAX_SPEED_MM_S}]", c.speed_mm_s)));
        }
        check_range(&mut out, "cut.power_pct", c.power_pct, 0.0, 100.0, "duty");
        check_range(&mut out, "cut.freq_hz", c.freq_hz, FREQ_RANGE_HZ.0, FREQ_RANGE_HZ.1, "frequency");
        check_range(&mut out, "cut.peak_current_pct", c.peak_current_pct, 0.0, 100.0, "peak current");
        check_range(&mut out, "cut.gas_pressure_bar", c.gas_pressure_bar, 0.0, MAX_PRESSURE_BAR, "gas pressure");
        for (p, v, what) in [
            ("cut.height_mm", c.height_mm, "cut height"),
            ("cut.up_height_mm", c.up_height_mm, "lift height"),
            ("cut.laser_on_delay_ms", c.laser_on_delay_ms, "laser-on delay"),
            ("cut.laser_off_before_delay_ms", c.laser_off_before_delay_ms, "pre-off delay"),
            ("cut.laser_off_after_delay_ms", c.laser_off_after_delay_ms, "post-off delay"),
        ] {
            check_nonneg(&mut out, p, v, what);
        }
        if !c.focus_mm.is_finite() {
            out.push(CraftIssue::error("cut.focus_mm", "focus must be finite"));
        }
        if self.process == ProcessType::FixedHeight || !self.follower.follow {
            check_nonneg(&mut out, "follower.fixed_height_cut_pos_mm", self.follower.fixed_height_cut_pos_mm, "fixed-height position");
        }
        let need = self.process.stages();
        if self.pierce.len() < need {
            out.push(CraftIssue::error("pierce", format!("process {:?} needs {need} pierce stage(s), {} given", self.process, self.pierce.len())));
        }
        if self.pierce.len() > 5 {
            out.push(CraftIssue::error("pierce", "at most five pierce stages are stored"));
        }
        for (i, s) in self.pierce.iter().enumerate() {
            let p = format!("pierce[{i}]");
            check_nonneg(&mut out, &format!("{p}.height_mm"), s.height_mm, "pierce height");
            check_range(&mut out, &format!("{p}.power_pct"), s.power_pct, 0.0, 100.0, "duty");
            check_range(&mut out, &format!("{p}.freq_hz"), s.freq_hz, FREQ_RANGE_HZ.0, FREQ_RANGE_HZ.1, "frequency");
            check_range(&mut out, &format!("{p}.peak_current_pct"), s.peak_current_pct, 0.0, 100.0, "peak current");
            check_range(&mut out, &format!("{p}.gas_pressure_bar"), s.gas_pressure_bar, 0.0, MAX_PRESSURE_BAR, "gas pressure");
            check_nonneg(&mut out, &format!("{p}.duration_ms"), s.duration_ms, "pierce time");
            check_nonneg(&mut out, &format!("{p}.before_laser_off_delay_ms"), s.before_laser_off_delay_ms, "dwell");
            check_nonneg(&mut out, &format!("{p}.after_laser_off_delay_ms"), s.after_laser_off_delay_ms, "blow time");
            if let Some(r) = s.focus_ramp {
                check_nonneg(&mut out, &format!("{p}.focus_ramp.time_ms"), r.time_ms, "focus ramp time");
            }
            if let Some(g) = s.gradual_time_ms {
                check_nonneg(&mut out, &format!("{p}.gradual_time_ms"), g, "gradual time");
                if i < need && (g - s.duration_ms).abs() > 1e-9 {
                    out.push(CraftIssue::warning(
                        &format!("{p}.gradual_time_ms"),
                        "the vendor editor keeps the gradual time equal to the pierce time",
                    ));
                }
            }
            if i < need && s.height_mm < c.height_mm {
                out.push(CraftIssue::warning(&format!("{p}.height_mm"), "pierce height below the cut height"));
            }
        }
        if c.height_mm > c.up_height_mm {
            out.push(CraftIssue::warning("cut.up_height_mm", "cut height above the lift height (legacy editor rule)"));
        }
        if self.follower.pre_drill_no_lift && !self.follower.pre_drill {
            out.push(CraftIssue::warning("follower.pre_drill_no_lift", "no-lift between pre-pierces without pre-pierce"));
        }
        if let Some(sp) = self.smooth_pierce {
            check_nonneg(&mut out, "smooth_pierce.height_mm", sp.height_mm, "smooth pierce height");
            check_nonneg(&mut out, "smooth_pierce.time_ms", sp.time_ms, "smooth pierce time");
            check_range(&mut out, "smooth_pierce.power_pct", sp.power_pct, 0.0, 100.0, "duty");
            check_range(&mut out, "smooth_pierce.freq_hz", sp.freq_hz, FREQ_RANGE_HZ.0, FREQ_RANGE_HZ.1, "frequency");
            check_range(&mut out, "smooth_pierce.peak_current_pct", sp.peak_current_pct, 0.0, 100.0, "peak current");
        }
        check_curve(&mut out, "power_curve", &self.power_curve, self.dynamic_power);
        check_curve(&mut out, "freq_curve", &self.freq_curve, self.dynamic_freq);
        check_slow_start(&mut out, &self.slow_start);
        out
    }

    /// True if [`validate`](Self::validate) reports no error (warnings allowed).
    pub fn is_valid(&self) -> bool {
        self.validate().iter().all(|i| i.severity != Severity::Error)
    }
}

impl Co2Craft {
    /// Check ranges.
    pub fn validate(&self) -> Vec<CraftIssue> {
        let mut out = Vec::new();
        if !self.speed_mm_s.is_finite() || self.speed_mm_s <= 0.0 || self.speed_mm_s > MAX_SPEED_MM_S {
            out.push(CraftIssue::error("speed_mm_s", format!("speed {} outside (0, {MAX_SPEED_MM_S}]", self.speed_mm_s)));
        }
        check_range(&mut out, "duty_pct", self.duty_pct, 0.0, 100.0, "duty");
        check_range(&mut out, "freq_hz", self.freq_hz, FREQ_RANGE_HZ.0, FREQ_RANGE_HZ.1, "frequency");
        check_range(&mut out, "gas_pressure_bar", self.gas_pressure_bar, 0.0, MAX_PRESSURE_BAR, "gas pressure");
        check_nonneg(&mut out, "laser_on_delay_ms", self.laser_on_delay_ms, "laser-on delay");
        check_nonneg(&mut out, "fixed_height_cut_pos_mm", self.fixed_height_cut_pos_mm, "fixed head position");
        check_curve(&mut out, "power_curve", &self.power_curve, self.dynamic_power);
        check_curve(&mut out, "freq_curve", &self.freq_curve, self.dynamic_freq);
        check_slow_start(&mut out, &self.slow_start);
        out
    }

    /// True if there is no error.
    pub fn is_valid(&self) -> bool {
        self.validate().iter().all(|i| i.severity != Severity::Error)
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    /// A three-stage oxygen pierce for thick carbon steel, shaped like the owner's documented
    /// preset slot 1 (values from `02-layer-params.md`, a worked example in the analysis).
    fn thick_carbon() -> FibreCraft {
        let stage = |h, p, f, pk, g, t, grad: Option<f64>, before, after| PierceStage {
            height_mm: h,
            power_pct: p,
            freq_hz: f,
            peak_current_pct: pk,
            gas: GasType::O2,
            gas_pressure_bar: g,
            duration_ms: t,
            gradual_time_ms: grad,
            before_laser_off_delay_ms: before,
            after_laser_off_delay_ms: after,
            ..PierceStage::default()
        };
        FibreCraft {
            name: "Carbon 10.0mm 4.0D F+12 O2".into(),
            nozzle: Some("4.0D".into()),
            process: ProcessType::Pierce3,
            cut: CutSettings {
                height_mm: 0.6,
                speed_mm_s: 11.0,
                power_pct: 88.0,
                freq_hz: 2000.0,
                peak_current_pct: 88.0,
                gas: GasType::O2,
                gas_pressure_bar: 0.6,
                laser_on_delay_ms: 10.0,
                up_height_mm: 15.0,
                ..CutSettings::default()
            },
            pierce: vec![
                stage(6.0, 65.0, 150.0, 60.0, 0.75, 1500.0, None, 500.0, 50.0),
                stage(18.0, 85.0, 200.0, 85.0, 1.2, 5500.0, Some(5500.0), 0.0, 0.0),
                stage(20.0, 80.0, 500.0, 80.0, 1.4, 350.0, None, 0.0, 0.0),
            ],
            power_curve: parse_curve_nodes("0,0,15,35,37,69,59,92,76,100,100,100").unwrap(),
            freq_curve: parse_curve_nodes("0,100,100,100").unwrap(),
            dynamic_power: false,
            ..FibreCraft::default()
        }
    }

    #[test]
    fn the_documented_example_is_valid() {
        let c = thick_carbon();
        let issues = c.validate();
        assert!(issues.is_empty(), "{issues:?}");
        assert!(c.is_valid());
        assert_eq!(c.process.stages(), 3);
        assert_eq!(c.cut.gas.code(), 1);
    }

    #[test]
    fn serde_round_trip_json() {
        let c = thick_carbon();
        let json = serde_json::to_string_pretty(&c).unwrap();
        assert!(json.contains("\"process\": \"pierce3\""));
        assert!(json.contains("\"gas\": \"o2\""));
        assert_eq!(serde_json::from_str::<FibreCraft>(&json).unwrap(), c);
        let table = CraftTable {
            fibre: vec![c.clone()],
            co2: vec![Co2Craft { name: "Acrylic 24mm".into(), speed_mm_s: 10.0, duty_pct: 20.0, gas: GasType::HighAir, ..Co2Craft::default() }],
        };
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(serde_json::from_str::<CraftTable>(&json).unwrap(), table);
        let tagged = serde_json::to_value(Craft::Fibre(c.clone())).unwrap();
        assert_eq!(tagged["head"], "fibre");
        assert_eq!(serde_json::from_value::<Craft>(tagged).unwrap(), Craft::Fibre(c));
        let co2 = serde_json::to_value(Craft::Co2(Co2Craft::default())).unwrap();
        assert_eq!(co2["head"], "co2");
    }

    #[test]
    fn defaults_are_the_vendor_descriptor_defaults() {
        let c = FibreCraft::default();
        assert_eq!((c.cut.height_mm, c.cut.speed_mm_s, c.cut.power_pct, c.cut.freq_hz), (1.0, 100.0, 100.0, 5000.0));
        assert_eq!((c.cut.up_height_mm, c.cut.gas_pressure_bar), (20.0, 5.0));
        assert!(c.follower.follow && c.follower.short_distance_no_lift);
        assert!(c.is_valid());
        let p = PierceStage::default();
        assert_eq!((p.height_mm, p.power_pct, p.freq_hz, p.duration_ms), (5.0, 50.0, 500.0, 1000.0));
        assert!(Co2Craft::default().is_valid());
        let t = CraftTable::default();
        assert_eq!((t.fibre.len(), t.co2.len()), (11, 11));
        assert!(t.validate().is_empty());
    }

    #[test]
    fn gas_and_process_codes() {
        for code in 0..=5u8 {
            assert_eq!(GasType::from_code(code).unwrap().code(), code);
        }
        assert_eq!(GasType::from_code(6), None);
        for code in 0..=4u8 {
            assert_eq!(ProcessType::from_code(code).unwrap().code(), code);
        }
        assert_eq!(ProcessType::from_code(5), None, "codes above 4 are not established");
        assert_eq!(ProcessType::DirectCut.stages(), 0);
        assert_eq!(ProcessType::FixedHeight.stages(), 0);
    }

    #[test]
    fn out_of_range_values_are_errors() {
        let mut c = FibreCraft::default();
        c.cut.freq_hz = 60000.0;
        c.cut.power_pct = 101.0;
        c.cut.speed_mm_s = 0.0;
        c.cut.gas_pressure_bar = 11.0;
        c.cut.height_mm = -1.0;
        c.cut.peak_current_pct = f64::NAN;
        let paths: Vec<String> = c.validate().into_iter().filter(|i| i.severity == Severity::Error).map(|i| i.path).collect();
        for p in ["cut.freq_hz", "cut.power_pct", "cut.speed_mm_s", "cut.gas_pressure_bar", "cut.height_mm", "cut.peak_current_pct"] {
            assert!(paths.iter().any(|x| x == p), "{p} not flagged: {paths:?}");
        }
        assert!(!c.is_valid());
        let mut c = Co2Craft::default();
        c.duty_pct = -1.0;
        c.freq_hz = 0.0;
        assert_eq!(c.validate().len(), 2);
    }

    #[test]
    fn pierce_stage_count_must_match_the_process() {
        let mut c = thick_carbon();
        c.pierce.truncate(2);
        let issues = c.validate();
        assert!(issues.iter().any(|i| i.path == "pierce" && i.severity == Severity::Error));
        c.process = ProcessType::Pierce2;
        assert!(c.is_valid());
        c.process = ProcessType::DirectCut;
        c.pierce.clear();
        assert!(c.is_valid());
        c.pierce = vec![PierceStage::default(); 6];
        assert!(!c.is_valid());
    }

    #[test]
    fn warnings_do_not_make_a_craft_invalid() {
        let mut c = thick_carbon();
        c.cut.up_height_mm = 0.5;
        c.pierce[1].gradual_time_ms = Some(100.0);
        c.pierce[0].height_mm = 0.1;
        c.follower.pre_drill_no_lift = true;
        let issues = c.validate();
        assert!(issues.len() >= 4);
        assert!(issues.iter().all(|i| i.severity == Severity::Warning), "{issues:?}");
        assert!(c.is_valid());
    }

    #[test]
    fn curves() {
        assert_eq!(parse_curve_nodes("").unwrap(), vec![]);
        let n = parse_curve_nodes("0,48,22,66,50,87,100,100").unwrap();
        assert_eq!(n.len(), 4);
        assert_eq!(n[1], CurveNode { speed_pct: 22.0, output_pct: 66.0 });
        assert_eq!(format_curve_nodes(&n), "0,48,22,66,50,87,100,100");
        assert!(parse_curve_nodes("0,1,2").is_err());
        assert!(parse_curve_nodes("a,b").is_err());
        assert!(parse_curve_nodes("0,inf").is_err());
        let mut c = FibreCraft::default();
        c.dynamic_power = true;
        assert!(!c.is_valid(), "enabled but empty");
        c.power_curve = parse_curve_nodes("10,20,5,30").unwrap();
        let issues = c.validate();
        assert!(issues.iter().any(|i| i.severity == Severity::Error && i.message.contains("decrease")));
        assert!(issues.iter().any(|i| i.severity == Severity::Warning));
        c.power_curve = parse_curve_nodes("0,0,150,50").unwrap();
        assert!(!c.is_valid());
    }

    #[test]
    fn table_lookup_and_paths() {
        let mut t = CraftTable::default();
        assert!(t.fibre_layer(3).is_some());
        assert!(t.fibre_layer(11).is_none());
        assert!(t.fibre_layer(-1).is_none());
        assert!(t.co2_layer(0).is_some());
        t.fibre[4].cut.freq_hz = 0.0;
        t.co2[2].duty_pct = 200.0;
        let paths: Vec<String> = t.validate().into_iter().map(|i| i.path).collect();
        assert_eq!(paths, vec!["fibre[4].cut.freq_hz".to_string(), "co2[2].duty_pct".to_string()]);
        t.fibre.push(FibreCraft::default());
        assert!(t.validate().iter().any(|i| i.path == "table"));
    }

    #[test]
    fn slow_start_is_checked() {
        let mut c = Co2Craft::default();
        c.slow_start = Some(SlowStart { length_mm: 1.0, speed_mm_s: 10.0 });
        assert!(c.is_valid());
        c.slow_start = Some(SlowStart { length_mm: -1.0, speed_mm_s: 0.0 });
        assert_eq!(c.validate().len(), 2);
    }

    #[test]
    fn fixed_height_needs_the_position() {
        let mut c = FibreCraft::default();
        c.process = ProcessType::FixedHeight;
        c.follower.follow = false;
        c.follower.fixed_height_cut_pos_mm = -3.0;
        assert!(!c.is_valid());
        c.follower.fixed_height_cut_pos_mm = 3.0;
        assert!(c.is_valid());
    }
}
