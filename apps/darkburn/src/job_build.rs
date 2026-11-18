#![allow(dead_code, reason = "the public API is for the machine side, wired when it lands")]
//! Turns the document into a job: maps layers, shapes and images into the job engine's input,
//! runs the planner and returns the toolpath. Nothing here needs a window.
//!
//! The machine side starts a job with these functions:
//!
//! - [`BuildInput`] gathers everything a job depends on (document, selection, project job
//!   settings, device, user origin). `job_cmds::build_input(&app)` fills it from the app.
//! - [`build_job`] runs on any thread with a cancel flag and a progress callback and returns a
//!   [`BuiltJob`]: `toolpath` in machine coordinates (what to send), `design` in design
//!   coordinates (what the preview draws), warnings and the work-area check.
//! - [`spawn_build`] does the same on a worker thread ([`BuildTask`]: poll, cancel, progress).
//! - [`BuiltJob::toolpath_from`] slices the job for "start from here".
//! - [`gcode_text`] / [`gcode_lines`] write G-code with the device's [`DeviceGcodeSettings`].
//! - [`build_frame`] makes the toolpath for framing (rectangle or rubber band).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};

use darkburn_core::kurbo::{Affine, BezPath, PathEl, Point, Rect};
use darkburn_core::layer::{self, CutSetting, LaserSelect, Layer, Mode};
use darkburn_core::{Document, ImageShape, Shape, ShapeId, ShapeKind};
use darkburn_job as job;
use darkburn_job::gcode::{AirCommand, Flavor, GcodeSettings, MarlinPower};
use darkburn_job::{
    BoundsReport, Corner, CutMode, FinishPosition, JobAnchor, JobError, JobInput, JobOptions, LayerInput, MachineGeometry, MotionSettings, Op,
    OriginSettings, PlannerSettings, Progress, ShapeInput, StartFrom, Toolpath, check_bounds,
};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// Curve flattening tolerance for jobs, mm.
pub const TOLERANCE: f64 = 0.01;
/// The version of the job settings saved in projects.
pub const SETTINGS_VERSION: u32 = 1;

// ----- Device ---------------------------------------------------------------------------------

/// The parts of a device the job depends on. The device profiles will convert into this.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobDevice {
    pub bed_w: f64,
    pub bed_h: f64,
    /// Where the machine's 0,0 is (ORIG-07).
    pub origin: Corner,
    /// CNC style negative coordinates (ORIG-08).
    pub negative_coords: bool,
    /// Swap X and Y (ORIG-09).
    pub swap_xy: bool,
    /// The machine has a Z axis: Z offset and step per pass are written.
    pub z_enabled: bool,
    /// Z moves go the other way (DS-04).
    pub z_reverse: bool,
    /// Where the red-dot pointer sits relative to the laser spot, machine axes, mm (ORIG-06, DS-03).
    pub pointer_offset: [f64; 2],
    /// Curve flattening tolerance for the job, mm (SET-36).
    pub tolerance: f64,
    /// Partly cut tabs are made of full-power pulses of this length, mm (DS-05). 0: the tab
    /// is cut at the tab power instead.
    pub tab_pulse_width: f64,
    /// The controller has no dynamic power: every layer is written with M3 (GRBL-02).
    pub m3_only: bool,
    /// The machine's job has no place for a pause on the spot (a Ruida job): start and end
    /// pauses and cut-through are left out, and the job says so in a warning.
    pub no_pauses: bool,
    /// The speed (mm/s) of the device's fast whitespace scan when it is on (DS-10): the time
    /// estimate counts blank moves inside scans at this speed, as the G-code is written.
    pub fast_whitespace: Option<f64>,
}

impl Default for JobDevice {
    fn default() -> Self {
        JobDevice {
            bed_w: 400.0,
            bed_h: 400.0,
            origin: Corner::BottomLeft,
            negative_coords: false,
            swap_xy: false,
            z_enabled: false,
            z_reverse: false,
            pointer_offset: [0.0, 0.0],
            tolerance: TOLERANCE,
            tab_pulse_width: 0.0,
            m3_only: false,
            no_pauses: false,
            fast_whitespace: None,
        }
    }
}

impl JobDevice {
    pub fn new(bed_w: f64, bed_h: f64) -> JobDevice {
        JobDevice { bed_w, bed_h, ..Default::default() }
    }

    pub fn geometry(&self) -> MachineGeometry {
        MachineGeometry { width: self.bed_w, height: self.bed_h, origin: self.origin, negative_coords: self.negative_coords, swap_xy: self.swap_xy }
    }

    /// The work area in design coordinates.
    pub fn area(&self) -> Rect {
        Rect::new(0.0, 0.0, self.bed_w, self.bed_h)
    }
}

/// Transfer mode for G-code devices (DS-13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transfer {
    #[default]
    Buffered,
    Synchronous,
}

/// G-code device settings (DS-09 to DS-15). The Devices dialog reuses [`gcode_settings_editor`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceGcodeSettings {
    pub flavor: Flavor,
    /// S value at 100 % power (DS-12).
    pub s_max: f64,
    pub air: AirCommand,
    pub baud_rate: u32,
    /// DS-09
    pub auto_home: bool,
    pub swap_xy: bool,
    /// DS-10
    pub fast_whitespace_scan: bool,
    pub fast_whitespace_speed: f64,
    /// DS-11
    pub jog_with_dollar_j: bool,
    pub dtr: bool,
    pub fire_button: bool,
    pub laser_on_when_framing: bool,
    /// DS-13
    pub transfer: Transfer,
    /// DS-14: lines written before and after the job.
    pub start_script: String,
    pub end_script: String,
    /// DS-15
    pub overscan_as_g0: bool,
    pub s_every_g1: bool,
    pub marlin_power: MarlinPower,
    pub tool_index: Option<u8>,
    /// Go back to the start (or origin) when the job ends (ORIG-10).
    pub return_to_finish: bool,
    /// Z moves are written as distances (DS-04).
    pub z_relative_only: bool,
    /// Skip Z moves to the height the head is already at (DS-04).
    pub z_optimize: bool,
}

impl Default for DeviceGcodeSettings {
    fn default() -> Self {
        DeviceGcodeSettings {
            flavor: Flavor::Grbl,
            s_max: 1000.0,
            air: AirCommand::M8,
            baud_rate: 115_200,
            auto_home: false,
            swap_xy: false,
            fast_whitespace_scan: false,
            fast_whitespace_speed: 100.0,
            jog_with_dollar_j: true,
            dtr: false,
            fire_button: false,
            laser_on_when_framing: false,
            transfer: Transfer::Buffered,
            start_script: String::new(),
            end_script: String::new(),
            overscan_as_g0: false,
            s_every_g1: false,
            marlin_power: MarlinPower::Inline,
            tool_index: None,
            return_to_finish: false,
            z_relative_only: false,
            z_optimize: true,
        }
    }
}

impl DeviceGcodeSettings {
    /// The writer settings for `darkburn_job::gcode`.
    pub fn to_gcode(&self) -> GcodeSettings {
        let mut g = GcodeSettings::new(self.flavor);
        g.s_max = self.s_max;
        g.air = self.air;
        g.overscan_as_g0 = self.overscan_as_g0;
        g.s_every_g1 = self.s_every_g1;
        g.marlin_power = self.marlin_power;
        g.tool_index = self.tool_index;
        g.z_relative_only = self.z_relative_only;
        g.z_always = !self.z_optimize;
        g.start_script = self.start_script.clone();
        if self.auto_home && matches!(self.flavor, Flavor::Grbl | Flavor::GrblHal | Flavor::FluidNc) {
            g.start_script = format!("$H\n{}", g.start_script);
        }
        g.end_script = self.end_script.clone();
        g
    }

    pub fn flavor_names() -> [&'static str; 6] {
        ["GRBL", "grblHAL", "FluidNC", "Marlin", "Smoothieware", "LinuxCNC"]
    }

    pub fn flavor_index(&self) -> usize {
        match self.flavor {
            Flavor::Grbl => 0,
            Flavor::GrblHal => 1,
            Flavor::FluidNc => 2,
            Flavor::Marlin => 3,
            Flavor::Smoothieware => 4,
            Flavor::LinuxCnc => 5,
        }
    }

    /// Pick a flavor by index; S max follows the flavor's usual value.
    pub fn set_flavor_index(&mut self, i: usize) {
        self.flavor = [Flavor::Grbl, Flavor::GrblHal, Flavor::FluidNc, Flavor::Marlin, Flavor::Smoothieware, Flavor::LinuxCnc][i.min(5)];
        self.s_max = GcodeSettings::new(self.flavor).s_max;
    }
}

/// The acceleration and speed numbers the time estimate and the preview use (DS-19, PREV-11).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdditionalSettings {
    /// Cutting acceleration, mm/s².
    pub accel: f64,
    /// Acceleration for fill and image, mm/s².
    pub engrave_accel: f64,
    pub rapid_speed: f64,
    pub max_speed: f64,
    pub junction_deviation: f64,
    pub z_speed: f64,
}

impl Default for AdditionalSettings {
    fn default() -> Self {
        let m = MotionSettings::default();
        AdditionalSettings {
            accel: m.accel,
            engrave_accel: m.engrave_accel,
            rapid_speed: m.rapid_speed,
            max_speed: m.max_speed,
            junction_deviation: m.junction_deviation,
            z_speed: m.z_speed,
        }
    }
}

impl AdditionalSettings {
    pub fn motion(&self) -> MotionSettings {
        MotionSettings {
            accel: self.accel.max(1.0),
            engrave_accel: self.engrave_accel.max(1.0),
            rapid_speed: self.rapid_speed.max(1.0),
            max_speed: self.max_speed.max(1.0),
            junction_deviation: self.junction_deviation.max(1e-4),
            z_speed: self.z_speed.max(0.1),
        }
    }
}

// ----- Settings saved with the project ----------------------------------------------------------

/// Where the job lands (LASER-11, ORIG-01 to ORIG-03).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartMode {
    /// Design coordinates are bed coordinates.
    #[default]
    Absolute,
    CurrentPosition,
    UserOrigin,
}

impl StartMode {
    pub const ALL: [StartMode; 3] = [StartMode::Absolute, StartMode::CurrentPosition, StartMode::UserOrigin];

    pub fn label(self) -> &'static str {
        match self {
            StartMode::Absolute => "Absolute coords",
            StartMode::CurrentPosition => "Current position",
            StartMode::UserOrigin => "User origin",
        }
    }

    pub fn from_name(name: &str) -> Option<StartMode> {
        Some(match name {
            "absolute" => StartMode::Absolute,
            "current_position" | "current" => StartMode::CurrentPosition,
            "user_origin" | "user" => StartMode::UserOrigin,
            _ => return None,
        })
    }
}

/// Job settings that travel with the project (PLAN-12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobProjectSettings {
    pub version: u32,
    /// Optimization settings (PLAN-01 to PLAN-10).
    pub planner: PlannerSettings,
    pub start_from: StartMode,
    /// The 9-point job origin (ORIG-04).
    pub anchor: JobAnchor,
    /// Anchor on the selection instead of the whole job (ORIG-05).
    pub use_selection_origin: bool,
    /// Cut selected graphics only (LASER-13).
    pub cut_selected: bool,
    /// Drop shapes that are not inside the work area (LASER-27).
    pub discard_out_of_bounds: bool,
    /// Show the last job's position on the canvas (LASER-15).
    pub show_last_position: bool,
    pub finish: FinishPosition,
}

impl Default for JobProjectSettings {
    fn default() -> Self {
        JobProjectSettings {
            version: SETTINGS_VERSION,
            planner: PlannerSettings::default(),
            start_from: StartMode::Absolute,
            anchor: JobAnchor::BottomLeft,
            use_selection_origin: false,
            cut_selected: false,
            discard_out_of_bounds: false,
            show_last_position: false,
            finish: FinishPosition::Stay,
        }
    }
}

impl JobProjectSettings {
    /// The settings saved in `doc`; `fallback` planner settings (the user's defaults) when the
    /// project has none or they cannot be read.
    pub fn from_doc(doc: &Document, fallback: &PlannerSettings) -> JobProjectSettings {
        match doc.job_settings.as_ref().and_then(|v| serde_json::from_value::<JobProjectSettings>(v.clone()).ok()) {
            Some(s) => s,
            None => JobProjectSettings { planner: fallback.clone(), ..Default::default() },
        }
    }

    /// The device's "return to the start when the job ends" turns the finish position on when
    /// the project has none of its own (ORIG-10).
    pub fn with_device_finish(mut self, return_to_finish: bool) -> JobProjectSettings {
        if return_to_finish && self.finish == FinishPosition::Stay {
            self.finish = FinishPosition::StartPosition;
        }
        self
    }

    /// Store in `doc` (the caller wraps this in an undo step).
    pub fn store(&self, doc: &mut Document) {
        doc.job_settings = serde_json::to_value(self).ok();
    }
}

// ----- Building -------------------------------------------------------------------------------

/// Everything a job depends on.
#[derive(Clone)]
pub struct BuildInput {
    pub doc: Document,
    pub selection: Vec<ShapeId>,
    pub settings: JobProjectSettings,
    pub device: JobDevice,
    /// The user origin in machine coordinates, if set.
    pub user_origin: Option<Point>,
    /// Where the head is now, machine coordinates (used for the work-area check when the job
    /// starts from the current position).
    pub head: Option<Point>,
    pub motion: AdditionalSettings,
    /// Scan tuning of the device; `None` uses the one the app has set.
    pub tuning: Option<crate::scan_offsets::ScanTuning>,
}

/// Why no job was built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    Cancelled,
    NothingToBurn,
    NothingSelected,
    NoUserOrigin,
    /// A shape or picture has a coordinate that is not a finite number.
    NotANumber,
    /// Cylinder correction is on and the job has pictures, which it cannot bend.
    PictureNotMapped,
    /// Nothing is left to burn because layers were left out for their size; the reasons, one
    /// per layer, already in the active language.
    TooBig(Vec<String>),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            BuildError::Cancelled => tr("job.error_cancelled", "job generation was cancelled"),
            BuildError::NothingToBurn => tr("job.error_nothing_to_burn", "there is nothing to burn: no shapes on layers that output"),
            BuildError::NothingSelected => tr("job.error_nothing_selected", "Cut selected graphics is on and nothing is selected"),
            BuildError::NoUserOrigin => tr("job.error_no_user_origin", "Start from is User origin, but no user origin is set"),
            BuildError::NotANumber => tr("job.error_not_a_number", "a shape or picture has a coordinate that is not a number"),
            BuildError::PictureNotMapped => {
                tr("job.error_picture", "cylinder correction cannot bend pictures: turn it off, or burn the pictures in a job of their own")
            }
            BuildError::TooBig(reasons) => {
                tr("job.error_too_big", "there is nothing left to burn: {reasons}").replace("{reasons}", &reasons.join(" "))
            }
        })
    }
}

impl std::error::Error for BuildError {}

/// The most scan lines the fill of one layer may have: 500 m of fill at 0.1 mm, far beyond any
/// machine. Past it the layer's shapes are left out of the job with a warning.
pub const MAX_FILL_ROWS: f64 = 5_000_000.0;

/// The most dots (dot mode) or perforation gaps the outlines of one layer may have: 500 m of
/// outline at 0.1 mm, far beyond any machine. Past it the layer's shapes are left out of the
/// job with a warning, as for [`MAX_FILL_ROWS`].
pub const MAX_LINE_PIECES: f64 = 5_000_000.0;

/// The line settings of the layer that split its outlines into pieces (dot mode, or
/// perforation), each as (step along the outline, passes, kerf offset, overcut). The step is
/// the dot spacing or one cut and skip, with the floors of [`cut_settings`]; passes are at
/// least 1, as [`map_settings_with`] gives the generator; dots get no overcut, as in the
/// generator.
fn piece_settings(l: &Layer) -> Vec<(f64, f64, f64, f64)> {
    l.settings
        .iter()
        .filter(|s| s.enabled && matches!(s.mode, Mode::Line | Mode::FillLine))
        .filter_map(|s| {
            let (step, overcut) = if s.dot_mode {
                (s.dot_spacing.max(0.01), 0.0)
            } else if s.perforation && s.perf_skip > 0.0 {
                (s.perf_cut.max(0.01) + s.perf_skip, s.overcut.max(0.0))
            } else {
                return None;
            };
            Some((step, s.passes.max(1) as f64, s.kerf, overcut))
        })
        .collect()
}

/// The reason a layer's dots or perforation were left out: `amount` pieces, past
/// [`MAX_LINE_PIECES`].
/// The warning for an image left out of the job, in the user's language.
fn image_left_out(label: &str, e: &crate::images::ImageError) -> String {
    let text = match e {
        crate::images::ImageError::TooLarge { dots, .. } => tr(
            "job.warn_image_too_large",
            "An image on layer {layer} was left out: image would be {amount} million dots at this resolution (the limit is {limit} million): lower the DPI.",
        )
        .replace("{amount}", &format!("{:.0}", *dots as f64 / 1e6))
        .replace("{limit}", &format!("{:.0}", crate::images::MAX_DOTS as f64 / 1e6)),
        crate::images::ImageError::Unreadable(why) => {
            tr("job.warn_image_unreadable", "An image on layer {layer} was left out: image cannot be read: {why}.").replace("{why}", why)
        }
    };
    text.replace("{layer}", label)
}

fn too_many_pieces_warning(li: usize, amount: f64) -> String {
    too_big_warning(
        tr(
            "job.warn_too_many_pieces",
            "The shapes on layer {layer} were left out: their dots or perforation would be {amount} million pieces (the limit is {limit} million). Check their size.",
        ),
        li,
        amount,
        MAX_LINE_PIECES,
    )
}

/// Leaves out the shapes of each layer whose real dots and perforation gaps, counted on the
/// outlines the generator cuts ([`job::line_pieces`]), pass [`MAX_LINE_PIECES`]: the whole
/// layer goes, never a part of it, so nothing different fires. `estimates` are the amounts the
/// early estimate gave, for the warning (the count stops just past the limit). Returns the
/// layers left out, each with its warning.
fn drop_too_many_pieces(layers: &mut [Option<LayerInput>], tolerance: f64, estimates: &[f64]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for entry in layers.iter_mut().flatten() {
        if entry.shapes.is_empty() {
            continue;
        }
        let n = job::line_pieces(entry, tolerance, MAX_LINE_PIECES);
        if n.is_nan() || n > MAX_LINE_PIECES {
            entry.shapes.clear();
            let li = entry.index;
            out.push((li, too_many_pieces_warning(li, n.max(estimates.get(li).copied().unwrap_or(0.0)))));
        }
    }
    out
}

/// The reason a layer's shapes were left out for their size, from its translated `text`.
fn too_big_warning(text: String, li: usize, amount: f64, limit: f64) -> String {
    text.replace("{layer}", &layer::layer_label(li))
        .replace("{amount}", &format!("{:.0}", amount / 1e6))
        .replace("{limit}", &format!("{:.0}", limit / 1e6))
}

/// The finest line interval of the layer's fill settings, or None when it does not fill.
fn fill_interval(l: &Layer) -> Option<f64> {
    l.settings
        .iter()
        .filter(|s| s.enabled && matches!(s.mode, Mode::Fill | Mode::FillLine | Mode::OffsetFill))
        .map(|s| s.interval.max(0.001))
        .min_by(f64::total_cmp)
}

/// A built job.
#[derive(Clone, Debug)]
pub struct BuiltJob {
    /// Machine coordinates (or relative to the start): what is sent or written.
    pub toolpath: Toolpath,
    /// The same operations in design coordinates, for the preview. Operation `i` is operation
    /// `i` of `toolpath`.
    pub design: Toolpath,
    /// Bounds of the burn in design coordinates.
    pub bounds: Rect,
    pub origin: OriginSettings,
    pub warnings: Vec<String>,
    /// The work-area check (empty when the job fits or cannot be checked).
    pub bounds_report: BoundsReport,
    /// Shapes left out because they were outside the work area.
    pub dropped_shapes: usize,
    /// The speed of the device's fast whitespace scan (see [`JobDevice::fast_whitespace`]).
    pub fast_whitespace: Option<f64>,
}

impl BuiltJob {
    pub fn out_of_bounds(&self) -> bool {
        !self.bounds_report.is_ok()
    }

    /// The job from operation `op` on ("Start here" and "Save from here").
    pub fn toolpath_from(&self, op: usize) -> Toolpath {
        self.toolpath.slice_from(op)
    }

    /// Time estimate and simulation in design coordinates.
    pub fn simulate(&self, motion: &AdditionalSettings) -> job::Simulation {
        match self.fast_whitespace {
            Some(speed) => job::sim::simulate(&fast_whitespace(&self.design, speed.max(1.0)), &motion.motion()),
            None => job::sim::simulate(&self.design, &motion.motion()),
        }
    }
}

struct Collected {
    job: JobInput,
    bounds: Option<Rect>,
    selection_bounds: Option<Rect>,
    /// Bounds the job is anchored on in place of its own, after shapes were dropped from it.
    anchor_bounds: Option<Rect>,
    frame_paths: Vec<BezPath>,
    /// Outlines on tool layers that are framed with the job.
    tool_frame: Vec<BezPath>,
    warnings: Vec<String>,
    dropped: usize,
    /// Why layers were left out for their size (also among `warnings`): when nothing else is
    /// left, the job is refused with these instead of a plain "nothing to burn".
    size_warnings: Vec<String>,
}

impl Collected {
    /// The error for a job with nothing left to burn: the size reasons when layers were left
    /// out for their size, else the plain [`BuildError::NothingToBurn`].
    fn nothing_to_burn(&self) -> BuildError {
        if self.size_warnings.is_empty() { BuildError::NothingToBurn } else { BuildError::TooBig(self.size_warnings.clone()) }
    }

    /// The origin settings of the build: anchored where "Remove what is outside the work area"
    /// placed the job when it dropped shapes.
    fn origin(&self, input: &BuildInput) -> Result<OriginSettings, BuildError> {
        let mut o = origin_for(input, self.selection_bounds)?;
        if self.anchor_bounds.is_some() {
            o.selection_bounds = self.anchor_bounds;
        }
        Ok(o)
    }
}

fn cut_mode(m: Mode) -> CutMode {
    match m {
        Mode::Line => CutMode::Line,
        Mode::Fill => CutMode::Fill,
        Mode::FillLine => CutMode::FillLine,
        Mode::OffsetFill => CutMode::OffsetFill,
        Mode::Image => CutMode::Image,
    }
}

fn overscan(s: &CutSetting) -> job::Overscan {
    if s.overscan <= 0.0 {
        job::Overscan::None
    } else if s.overscan_percent {
        job::Overscan::Percent(s.overscan)
    } else {
        job::Overscan::Distance(s.overscan)
    }
}

/// Map one cut setting (a layer's main settings or a sub-layer) to the job engine's.
pub fn map_settings(s: &CutSetting, layer: &Layer, z_enabled: bool) -> job::CutSettings {
    map_settings_with(s, layer, z_enabled, &crate::scan_offsets::ScanTuning::default())
}

/// The tab settings of a cut setting as the job engine takes them.
pub fn tab_settings(s: &CutSetting) -> job::TabSettings {
    job::TabSettings {
        enabled: s.tabs.enabled,
        auto: s.tabs.auto_spacing,
        spacing: s.tabs.spacing.max(0.1),
        min_count: s.tabs.min_count,
        max_count: s.tabs.max_count.max(s.tabs.min_count),
        size: s.tabs.size.max(0.0),
        tab_power: s.tabs.partial.then_some(s.tabs.tab_power.clamp(0.0, 100.0)),
        skip_inner: s.tabs.skip_inner,
        // The device's tab pulse width is filled in by the job builder.
        pulse_width: 0.0,
    }
}

/// [`map_settings`] with the device's scan tuning (fast skip over blank stretches, DS-08).
pub fn map_settings_with(s: &CutSetting, layer: &Layer, z_enabled: bool, tuning: &crate::scan_offsets::ScanTuning) -> job::CutSettings {
    let lead = |l: &layer::Lead| {
        l.enabled.then_some(job::Lead {
            length: l.length.max(0.0),
            angle_deg: l.angle,
            style: match l.style {
                layer::LeadStyle::Line => job::LeadStyle::Line,
                layer::LeadStyle::Arc => job::LeadStyle::Arc,
            },
        })
    };
    let mut c = job::CutSettings {
        enabled: s.enabled,
        mode: cut_mode(s.mode),
        speed: s.speed.max(0.01),
        max_power: s.max_power.clamp(0.0, 100.0),
        // A min power above the max is held to the max: the layer's max is never passed.
        min_power: s.min_power.clamp(0.0, 100.0).min(s.max_power.clamp(0.0, 100.0)),
        passes: s.passes.max(1),
        air: layer.air,
        laser: if s.laser == LaserSelect::Laser2 { 2 } else { 1 },
        constant_power: s.constant_power,
        dsp: job::DspSettings {
            pwm_hz: s.pwm_override.then(|| (s.pwm_khz.clamp(0.1, 1000.0) * 1000.0).round() as u32),
            ppi: s.ppi_enabled.then_some(s.ppi.max(1.0)),
            u_offset: s.u_offset,
            power2: (s.laser != LaserSelect::Laser1)
                .then_some([s.min_power_2.clamp(0.0, 100.0).min(s.max_power_2.clamp(0.0, 100.0)), s.max_power_2.clamp(0.0, 100.0)]),
        },
        z: job::ZSettings { enabled: z_enabled && (s.z_offset != 0.0 || s.z_step != 0.0), offset: s.z_offset, step_per_pass: s.z_step },
        ..job::CutSettings::default()
    };
    c.line = job::LineSettings {
        perforation: s.perforation.then_some(job::Perforation { cut: s.perf_cut.max(0.01), skip: s.perf_skip.max(0.0) }),
        tabs: tab_settings(s),
        start_pause_ms: s.start_pause_ms.max(0.0),
        end_pause_ms: s.end_pause_ms.max(0.0),
        cut_through_start: s.cut_through_start,
        cut_through_end: s.cut_through_end,
        pause_power: s.cut_through_power.clamp(0.0, 100.0),
        overcut: s.overcut.max(0.0),
        lead_in: lead(&s.lead_in),
        lead_out: lead(&s.lead_out),
        dot_mode: s.dot_mode.then_some(job::DotMode { time_ms: s.dot_time_ms.max(0.0), spacing: s.dot_spacing.max(0.01) }),
    };
    c.fill = job::FillSettings {
        interval: s.interval.max(0.001),
        angle_deg: s.scan_angle,
        angle_step_per_pass: s.angle_increment,
        bidirectional: s.bidirectional,
        cross_hatch: s.cross_hatch,
        overscan: overscan(s),
        fill_rule: job::FillRule::EvenOdd,
        grouping: match s.fill_grouping {
            layer::FillGrouping::AllAtOnce => job::FillGrouping::AllAtOnce,
            layer::FillGrouping::GroupsTogether => job::FillGrouping::GroupsTogether,
            layer::FillGrouping::Individually => job::FillGrouping::Individually,
        },
        flood_fill: s.flood_fill,
        ramp_length: s.ramp_length.max(0.0),
        ramp_outer_edge: s.ramp_outer,
        // The device's scanning offset table moves forward and reverse lines apart (DS-08).
        direction_offset: if s.bidirectional { tuning.offsets.offset_at(s.speed) } else { 0.0 },
        ..job::FillSettings::default()
    };
    c.image = job::ImageSettings {
        bidirectional: s.bidirectional,
        overscan: overscan(s),
        skip_blanks: tuning.fast_skip,
        rapid_gap: tuning.skip_mm.max(0.1),
    };
    c
}

/// Pixels to the shape's local millimetres: the image is centred on the local origin, y up.
fn pixel_to_local(img: &ImageShape, px_w: u32, px_h: u32) -> Affine {
    let sx = img.width / f64::from(px_w.max(1));
    let sy = img.height / f64::from(px_h.max(1));
    Affine::new([sx, 0.0, 0.0, -sy, -img.width / 2.0, img.height / 2.0])
}

/// Scan lines of the images of one layer, using the layer's image settings. `map` is the
/// print-and-cut and rotary mapping of the job, which moves the pictures with the outlines.
fn image_inputs(
    doc: &Document,
    leaves: &[(&Shape, Option<u32>)],
    map: Affine,
    s: &CutSetting,
    tuning: &crate::scan_offsets::ScanTuning,
    warnings: &mut Vec<String>,
) -> Vec<job::ImageInput> {
    let settings = darkburn_raster::ImageSettings {
        mode: crate::images::engine_mode(s),
        line_interval_mm: s.interval.max(0.001),
        scan_angle_deg: s.scan_angle,
        negative: s.negative,
        linear_light: true,
    };
    let scan = darkburn_raster::ScanSettings {
        bidirectional: true,
        overscan_mm: 0.0,
        blank_skip_mm: None,
        dot_width_mm: s.dot_width_correction.max(0.0),
        min_power: 0.4,
        max_power: 100.0,
    };
    // (dots, group, priority, power scale, corners of the picture)
    let mut processed: Vec<(darkburn_raster::ProcessedImage, Option<u32>, i32, f64, Vec<Point>)> = Vec::new();
    for (shape, group) in leaves {
        let ShapeKind::Image(img) = &shape.kind else { continue };
        let label = layer::layer_label(shape.layer.min(layer::LAYER_COUNT - 1));
        // An image too fine to scan is left out before anything that big is allocated.
        let (w, h) = crate::images::size_mm(shape, img);
        let bb = shape.bbox();
        let dots = crate::images::dots_for(bb.width().max(w), bb.height().max(h), settings.line_interval_mm);
        if dots > crate::images::MAX_DOTS {
            warnings.push(image_left_out(&label, &crate::images::ImageError::TooLarge { dots, shape: shape.id.0 }));
            continue;
        }
        // The pixels with the image's crop applied (outside the crop nothing burns).
        let gray = match crate::images::source_gray(doc, img) {
            Ok(g) => g,
            Err(e) => {
                warnings.push(image_left_out(&label, &e));
                continue;
            }
        };
        let to_mm = map * shape.transform * pixel_to_local(img, gray.width, gray.height);
        let p = darkburn_raster::process_image(&gray, to_mm, &crate::images::adjustments(&img.adjust), &settings);
        let outline: Vec<Point> = job::geom::flatten(&(map * shape.outline()), TOLERANCE).into_iter().flat_map(|poly| poly.pts).collect();
        processed.push((p, *group, shape.priority, (shape.power_scale / 100.0).clamp(0.0, 1.0), outline));
    }
    let mut out = Vec::new();
    // The device's scanning offset table moves forward and reverse lines apart (DS-08). The job
    // engine applies it, because only it knows which way each line ends up being scanned.
    let direction_offset = if s.bidirectional { tuning.offsets.offset_at(s.speed) } else { 0.0 };
    let build =
        |plans: Vec<darkburn_raster::ScanPlan>, group: Option<u32>, priority: i32, scale: f64, outline: &[Point], out: &mut Vec<job::ImageInput>| {
            for plan in plans {
                let lines = plan
                    .lines
                    .iter()
                    .map(|l| {
                        let mut runs: Vec<(f64, f64, f64)> =
                            l.spans.iter().flat_map(|sp| sp.runs.iter()).map(|r| (r.x0, r.x1, f64::from(r.power) / 100.0)).collect();
                        runs.sort_by(|a, b| a.0.total_cmp(&b.0));
                        job::Scanline { y: l.y, runs }
                    })
                    .collect();
                let mut input = job::ImageInput::new(lines);
                input.transform = plan.scan_to_mm();
                input.group = group;
                input.priority = priority;
                input.direction_offset = direction_offset;
                input.outline = outline.to_vec();
                input.power_scale = scale;
                out.push(input);
            }
        };
    if s.merge_images && processed.iter().all(|p| (p.3 - processed[0].3).abs() < 1e-9) && !processed.is_empty() {
        let refs: Vec<&darkburn_raster::ProcessedImage> = processed.iter().map(|p| &p.0).collect();
        let group = processed[0].1;
        let prio = processed.iter().map(|p| p.2).min().unwrap_or(0);
        let outline: Vec<Point> = processed.iter().flat_map(|p| p.4.iter().copied()).collect();
        build(darkburn_raster::scan_plans(&refs, &scan), group, prio, processed[0].3, &outline, &mut out);
    } else {
        for p in &processed {
            build(darkburn_raster::scan_plans(&[&p.0], &scan), p.1, p.2, p.3, &p.4, &mut out);
        }
    }
    out
}

/// The kerf offset each line sub-layer of `layer` cuts with, as (index into its settings,
/// kerf). Fills are never offset.
fn kerfs_of(layer: &Layer) -> Vec<(usize, f64)> {
    layer
        .settings
        .iter()
        .enumerate()
        .filter(|(_, s)| s.enabled && matches!(s.mode, Mode::Line | Mode::FillLine) && s.kerf != 0.0)
        .map(|(i, s)| (i, s.kerf))
        .collect()
}

/// The closed outlines of the shapes on the layers that cut lines, each known by its top shape
/// and its place among its leaves: a shape drawn inside one of them is a hole, and its kerf
/// goes the other way.
pub(crate) fn kerf_outlines(doc: &Document, tolerance: f64) -> Vec<((ShapeId, usize), Rect, Vec<Point>)> {
    let cuts_lines = |l: &Layer| l.settings.iter().any(|s| s.enabled && s.passes > 0 && matches!(s.mode, Mode::Line | Mode::FillLine));
    let mut out = Vec::new();
    for top in doc.iter() {
        for (k, leaf) in top.leaves().into_iter().enumerate() {
            let l = doc.layer(leaf.layer.min(layer::LAYER_COUNT - 1));
            if !l.outputs() || leaf.is_image() || !cuts_lines(l) {
                continue;
            }
            for poly in job::geom::flatten(&leaf.outline(), tolerance.max(0.01)) {
                if poly.closed {
                    out.push(((top.id, k), job::geom::bbox(&poly.pts), poly.pts));
                }
            }
        }
    }
    out
}

/// How many of `outlines` (from [`kerf_outlines`]) the leaf `no`, drawn as `path` within
/// `bbox`, lies inside: odd means a hole.
pub(crate) fn kerf_depth(outlines: &[((ShapeId, usize), Rect, Vec<Point>)], no: (ShapeId, usize), bbox: Rect, path: &BezPath) -> usize {
    match path.elements().first() {
        Some(PathEl::MoveTo(p0)) => outlines
            .iter()
            .filter(|(o, r, pts)| {
                *o != no && r.area() > bbox.area() + 1e-9 && job::geom::rect_inside(bbox, *r) && job::geom::point_in_polygon(*p0, pts)
            })
            .count(),
        _ => 0,
    }
}

fn is_outside(bbox: Rect, area: Rect) -> bool {
    let tol = 1e-6;
    bbox.x0 < area.x0 - tol || bbox.y0 < area.y0 - tol || bbox.x1 > area.x1 + tol || bbox.y1 > area.y1 + tol
}

/// The bounds of a shape where the job puts it: after the cylinder, rotary and print-and-cut
/// mapping of the project (as drawn when there is none).
fn placed_bbox(s: &Shape, map: Option<&crate::vision_state::JobMap>) -> Rect {
    let Some(m) = map else { return s.bbox() };
    let path = s.outline();
    if path.elements().is_empty() {
        let o = m.apply_point(s.transform * Point::ZERO);
        return Rect::from_points(o, o);
    }
    darkburn_core::kurbo::Shape::bounding_box(&m.apply(path))
}

/// Where the start mode puts design coordinates on the bed, as a map from design to bed
/// (design) coordinates, for a job with these bounds. As drawn for absolute jobs, and for jobs
/// from the current position while the head position is unknown.
fn bed_placement(input: &BuildInput, selection_bounds: Option<Rect>, job_bounds: Rect) -> Affine {
    let Ok(origin) = origin_for(input, selection_bounds) else { return Affine::IDENTITY };
    let to_machine = match (origin.start_from, input.head) {
        (StartFrom::Absolute, _) | (StartFrom::CurrentPosition, None) => return Affine::IDENTITY,
        (StartFrom::CurrentPosition, Some(h)) => Affine::translate(h.to_vec2()) * origin.transform(job_bounds),
        (StartFrom::UserOrigin(_), _) => origin.transform(job_bounds),
    };
    origin.machine.affine().inverse() * to_machine
}

fn collect(input: &BuildInput, cancel: Option<&AtomicBool>, report: &mut dyn FnMut(f32)) -> Result<Collected, BuildError> {
    // Text hook: variable text, QR codes and barcodes are evaluated for this job.
    let prepared = crate::text_render::prepare_for_output(&input.doc);
    let doc = &prepared;
    let st = &input.settings;
    if st.cut_selected && input.selection.is_empty() {
        return Err(BuildError::NothingSelected);
    }
    let cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let mut warnings = Vec::new();
    let area = input.device.area();
    let order_pos: HashMap<usize, usize> = doc.layer_order.iter().enumerate().map(|(i, l)| (*l, i)).collect();

    let mut frame_paths = Vec::new();
    // Cylinder, rotary and print-and-cut mapping set up in the project (vision_state.rs).
    let vision_map = crate::vision_state::JobMap::from_doc(doc);
    // Selection bounds for "use selection origin" (all selected shapes, burning or not), where
    // the job puts them: the anchor is measured on the same mapped shapes as the job bounds.
    let placed = |s: &Shape| placed_bbox(s, vision_map.as_ref());
    // A group is measured leaf by leaf, as it is mapped below: the cylinder leaves alone only
    // the leaves it cannot reach, not the whole group. A group without leaves keeps its origin.
    let selection_bounds = input
        .selection
        .iter()
        .filter_map(|id| doc.get(*id))
        .flat_map(|s| {
            let leaves = s.leaves();
            if leaves.is_empty() { vec![s.clone()] } else { leaves }
        })
        .map(|s| placed(&s))
        .reduce(|a, b| a.union(b));
    let tuning = input.tuning.clone().unwrap_or_else(crate::scan_offsets::current);

    let tops: Vec<&Shape> = doc.iter().filter(|s| !st.cut_selected || input.selection.contains(&s.id)).collect();
    let total = tops.len().max(1);
    // Kerf: a shape drawn inside another shape that is cut is a hole, and its kerf goes the
    // other way. The shape around it may be on another layer (holes are often given a layer of
    // their own to cut them first). These are the closed outlines of the shapes on the layers
    // that cut lines, each with the number of its shape; they are only needed when some layer
    // has a kerf offset. With "Cut selected graphics" the shapes that are not selected still
    // count here (a selected hole in an unselected outline is still a hole); only the selected
    // ones are cut. Each outline is known by its top shape and its place among its leaves.
    let any_kerf = (0..layer::LAYER_COUNT).any(|li| doc.layer(li).outputs() && !kerfs_of(doc.layer(li)).is_empty());
    let kerf_outlines = if any_kerf { kerf_outlines(doc, input.device.tolerance) } else { Vec::new() };
    // A fill is scanned line by line across everything on its layer. Shapes scaled up by
    // accident to kilometres would be more lines than there is memory for, so a layer whose
    // fill is that large is left out before any of it is worked out.
    let mut fill_span: Vec<Option<Rect>> = vec![None; layer::LAYER_COUNT];
    for top in &tops {
        for leaf in top.leaves() {
            let li = leaf.layer.min(layer::LAYER_COUNT - 1);
            let l = doc.layer(li);
            if l.outputs() && !leaf.is_image() && fill_interval(l).is_some() {
                let b = leaf.bbox();
                fill_span[li] = Some(fill_span[li].map_or(b, |r| r.union(b)));
            }
        }
    }
    // Dot mode and perforation split every outline into pieces as long as their step, so an
    // outline of kilometres (or one scaled up by accident) would be more pieces than there is
    // memory for: such a layer is left out the same way.
    // The estimate is taken on what the generator splits: each setting with a kerf cuts the
    // kerf-offset outline, at most 2π·|kerf| longer for each closed subpath (offsets have round
    // corners), and a vision mapping (cylinder, rotary, print and cut) is measured on the
    // mapped outline, its stretch applied to the kerf allowance too. Every such setting and
    // every pass make their own pieces, each at least one per outline (a dot at its start, or
    // one gap), and a perforated closed outline is cut on over its start by the overcut. This
    // is only a quick early exit: the real pieces are counted on the outlines the generator
    // cuts once they are made (see drop_too_many_pieces), which also covers any mapping.
    let mut pieces = [0.0_f64; layer::LAYER_COUNT];
    for top in &tops {
        for leaf in top.leaves() {
            let li = leaf.layer.min(layer::LAYER_COUNT - 1);
            let l = doc.layer(li);
            if !l.outputs() || leaf.is_image() {
                continue;
            }
            let splits = piece_settings(l);
            if splits.is_empty() {
                continue;
            }
            let outline = leaf.outline();
            let drawn = darkburn_geom::path::length(&outline);
            let subpaths = outline.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count() as f64;
            let estimate = |len: f64, stretch: f64| -> f64 {
                splits
                    .iter()
                    .map(|(step, passes, kerf, overcut)| {
                        let cut = len + stretch * std::f64::consts::TAU * kerf.abs() * subpaths;
                        ((cut + (overcut * subpaths).min(cut)) / step + subpaths) * passes
                    })
                    .sum()
            };
            let mut here = estimate(drawn, 1.0);
            // A mapped outline is only measured while the drawn one is within the limit (an
            // outline of kilometres is left out anyway, and mapping it would take as long).
            if let Some(m) = &vision_map
                && here <= MAX_LINE_PIECES
                && drawn > 0.0
            {
                let mapped = darkburn_geom::path::length(&m.apply(outline));
                let stretch = (mapped / drawn).max(1.0);
                here = estimate(mapped.max(drawn), stretch);
            }
            pieces[li] += here;
        }
    }
    let mut too_big = [false; layer::LAYER_COUNT];
    let mut size_warnings = Vec::new();
    for li in 0..layer::LAYER_COUNT {
        if pieces[li].is_nan() || pieces[li] > MAX_LINE_PIECES {
            too_big[li] = true;
            size_warnings.push(too_many_pieces_warning(li, pieces[li]));
        }
    }
    for li in 0..layer::LAYER_COUNT {
        if too_big[li] {
            continue;
        }
        let (Some(span), Some(interval)) = (fill_span[li], fill_interval(doc.layer(li))) else { continue };
        let rows = span.width().max(span.height()) / interval;
        // (A size that is not a number is not let through either.)
        if rows.is_nan() || rows > MAX_FILL_ROWS {
            too_big[li] = true;
            size_warnings.push(too_big_warning(
                tr(
                    "job.warn_too_many_rows",
                    "The shapes on layer {layer} were left out: the fill would be {amount} million scan lines (the limit is {limit} million). Check their size.",
                ),
                li,
                rows,
                MAX_FILL_ROWS,
            ));
        }
    }
    warnings.extend(size_warnings.iter().cloned());
    // "Remove what is outside the work area" tests each shape where the start mode puts it on
    // the bed: from the user origin or the head, the anchor of the whole job (all the shapes
    // that could burn) goes there. Absolute jobs, and jobs from the current position while the
    // head position is unknown, are measured where they are mapped.
    let placing = st.discard_out_of_bounds
        && match st.start_from {
            StartMode::Absolute => false,
            StartMode::CurrentPosition => input.head.is_some(),
            StartMode::UserOrigin => true,
        };
    // The shapes are placed, tested against the work area and collected; when the real count
    // below leaves a layer out, all of it is done again without that layer, so a layer left
    // out never moves the job nor makes other shapes fall outside the work area. Each round
    // leaves out at least one more layer, so this ends.
    let warnings_before = warnings.len();
    let mut real_drops: Vec<String> = Vec::new();
    let (mut layers, leaves_by_layer, line_frames, tool_frame, kerf_lost, place_bounds, dropped) = loop {
        warnings.truncate(warnings_before);
        let mut dropped = 0usize;
        let mut layers: Vec<Option<LayerInput>> = (0..layer::LAYER_COUNT).map(|_| None).collect();
        let mut leaves_by_layer: Vec<Vec<(Shape, Option<u32>)>> = (0..layer::LAYER_COUNT).map(|_| Vec::new()).collect();
        // The outlines of each layer, framed unless the layer is left out for its size.
        let mut line_frames: Vec<Vec<BezPath>> = (0..layer::LAYER_COUNT).map(|_| Vec::new()).collect();
        let mut tool_frame: Vec<BezPath> = Vec::new();
        let mut kerf_lost = [0usize; layer::LAYER_COUNT];
        let place_bounds = if !placing {
            None
        } else {
            tops.iter()
                .flat_map(|t| t.leaves())
                .filter(|l| {
                    // A layer left out for its size is not part of the job and does not move it.
                    let li = l.layer.min(layer::LAYER_COUNT - 1);
                    doc.layer(li).outputs() && !(too_big[li] && !l.is_image())
                })
                .map(|l| placed(&l))
                .reduce(|a, b| a.union(b))
        };
        let bed_place = place_bounds.map_or(Affine::IDENTITY, |b| bed_placement(input, selection_bounds, b));
        for (n, top) in tops.iter().enumerate() {
            if cancelled() {
                return Err(BuildError::Cancelled);
            }
            report(0.25 * n as f32 / total as f32);
            let group = top.is_group().then_some((top.id.0 & 0xffff_ffff) as u32);
            for (k, leaf) in top.leaves().into_iter().enumerate() {
                let li = leaf.layer.min(layer::LAYER_COUNT - 1);
                let l = doc.layer(li);
                if !l.outputs() {
                    // Tool layers never burn, but may be framed with the job (LAYER-02).
                    if l.is_tool() && l.frame && !leaf.is_image() {
                        // Mapped like the shapes that burn, so the frame stays with the job.
                        let outline = leaf.outline();
                        tool_frame.push(match &vision_map {
                            Some(m) => m.apply(outline),
                            None => outline,
                        });
                    }
                    continue;
                }
                if too_big[li] && !leaf.is_image() {
                    continue;
                }
                let bbox = leaf.bbox();
                // Measured where the job puts the shape (print and cut and rotary move it, and so
                // does the start mode).
                if st.discard_out_of_bounds && is_outside(bed_place.transform_rect_bbox(placed(&leaf)), area) {
                    dropped += 1;
                    continue;
                }
                if matches!(&leaf.kind, ShapeKind::Text(t) if t.backup.is_none()) {
                    let text = match &leaf.kind {
                        ShapeKind::Text(t) => t.text.as_str(),
                        _ => "",
                    };
                    warnings.push(tr("job.warn_text_no_outline", "Text \"{text}\" has no outline yet and was left out.").replace("{text}", text));
                    continue;
                }
                let entry = layers[li].get_or_insert_with(|| {
                    let mut settings: Vec<job::CutSettings> = l
                        .settings
                        .iter()
                        .map(|s| {
                            let mut c = map_settings_with(s, l, input.device.z_enabled, &tuning);
                            c.constant_power |= input.device.m3_only;
                            c
                        })
                        .collect();
                    for s in &mut settings {
                        s.line.tabs.pulse_width = input.device.tab_pulse_width.max(0.0);
                    }
                    LayerInput {
                        index: li,
                        name: if l.name.is_empty() { layer::layer_label(li) } else { l.name.clone() },
                        priority: order_pos.get(&li).copied().unwrap_or(li) as i32,
                        output: true,
                        settings,
                        shapes: Vec::new(),
                        images: Vec::new(),
                    }
                });
                if leaf.is_image() {
                    leaves_by_layer[li].push((leaf, group));
                    continue;
                }
                let mut path = leaf.outline();
                if path.elements().is_empty() {
                    continue;
                }
                // Each line sub-layer with a kerf offset cuts its own outline; fills keep the drawn one.
                let kerfs = kerfs_of(l);
                let mut line_paths: Vec<(usize, BezPath)> = Vec::new();
                if !kerfs.is_empty() {
                    // How many other cut shapes this one lies inside: odd means a hole.
                    let depth = kerf_depth(&kerf_outlines, (top.id, k), bbox, &path);
                    let mut lost_most = 0;
                    for (setting, kerf) in kerfs {
                        let kerf = if depth % 2 == 1 { -kerf } else { kerf };
                        let (offset, lost) = darkburn_geom::offset::kerf_offset_counted(&path, kerf, input.device.tolerance);
                        lost_most = lost_most.max(lost);
                        line_paths.push((setting, offset));
                    }
                    kerf_lost[li] += lost_most;
                }
                if let Some(m) = &vision_map {
                    path = m.apply(path);
                    for (_, p) in &mut line_paths {
                        *p = m.apply(std::mem::take(p));
                    }
                }
                let mut si = ShapeInput::new(path);
                si.line_paths = line_paths;
                {
                    let used = si.outlines_used(&entry.settings);
                    if used.iter().all(|p| p.elements().is_empty()) {
                        // Nothing of the shape is left to cut.
                        continue;
                    }
                    line_frames[li].extend(used.into_iter().filter(|p| !p.elements().is_empty()));
                }
                // Tabs and forced start points are stored in the shape's local coordinates; they
                // go through the same mapping as the outline (the kerf offset keeps them: each
                // snaps to the nearest point of the path that is cut).
                let place = |p: &[f64; 2]| {
                    let q = leaf.transform * Point::new(p[0], p[1]);
                    vision_map.as_ref().map_or(q, |m| m.apply_point(q))
                };
                si.start_point = leaf.start_point.as_ref().map(place);
                si.extra_start_points = leaf.extra_start_points.iter().map(place).collect();
                si.direction = match leaf.direction {
                    darkburn_core::CutDirection::Auto => None,
                    darkburn_core::CutDirection::Clockwise => Some(job::ForcedDirection::Clockwise),
                    darkburn_core::CutDirection::CounterClockwise => Some(job::ForcedDirection::CounterClockwise),
                    darkburn_core::CutDirection::Reversed => Some(job::ForcedDirection::Reversed),
                };
                si.tabs = leaf.tabs.iter().map(place).collect();
                si.group = group;
                si.priority = leaf.priority;
                si.power_scale = (leaf.power_scale / 100.0).clamp(0.0, 1.0);
                entry.shapes.push(si);
            }
        }
        // The estimate above is only an early exit: the real pieces are counted on the outlines
        // as the generator cuts them (kerf and mapping applied), and a layer past the limit is
        // left out whole, before anything of it is generated, and the job is placed again.
        let drops = drop_too_many_pieces(&mut layers, input.device.tolerance, &pieces);
        if drops.is_empty() {
            break (layers, leaves_by_layer, line_frames, tool_frame, kerf_lost, place_bounds, dropped);
        }
        for (li, why) in drops {
            too_big[li] = true;
            real_drops.push(why.clone());
            size_warnings.push(why);
        }
    };
    warnings.extend(real_drops);
    frame_paths.extend(line_frames.into_iter().flatten());
    for (li, lost) in kerf_lost.iter().enumerate() {
        if *lost > 0 {
            let note = if *lost == 1 {
                tr("job.warn_kerf_lost_one", "Layer {layer}: 1 shape or hole smaller than the kerf offset was left out.")
            } else {
                tr("job.warn_kerf_lost", "Layer {layer}: {n} shapes or holes smaller than the kerf offset were left out.")
            };
            warnings.push(note.replace("{layer}", &layer::layer_label(li)).replace("{n}", &lost.to_string()));
        }
    }

    if let Some(m) = &vision_map {
        warnings.extend(m.warnings.take());
    }

    // Images, layer by layer.
    for li in 0..layer::LAYER_COUNT {
        if leaves_by_layer[li].is_empty() {
            continue;
        }
        if cancelled() {
            return Err(BuildError::Cancelled);
        }
        report(0.25 + 0.05 * li as f32 / layer::LAYER_COUNT as f32);
        let l = doc.layer(li);
        let Some(entry) = layers[li].as_mut() else { continue };
        // Every image sub-layer that is on scans the pictures with its own settings
        // (interval, angle, mode), in sub-layer order, as two fill or line sub-layers do.
        let image_settings: Vec<(usize, &CutSetting)> =
            l.settings.iter().enumerate().filter(|(_, s)| s.enabled && s.mode == Mode::Image).collect();
        if image_settings.is_empty() {
            warnings.push(
                tr("job.warn_not_image_mode", "Layer {layer} holds images but is not in Image mode; they were left out.")
                    .replace("{layer}", &layer::layer_label(li)),
            );
            continue;
        }
        // Pictures go through the same print-and-cut and rotary mapping as the outlines.
        // Cylinder correction bends paths, which a picture cannot follow: refused rather
        // than burned in the wrong place.
        let Some(map) = vision_map.as_ref().map_or(Some(Affine::IDENTITY), |m| m.affine()) else {
            return Err(BuildError::PictureNotMapped);
        };
        let refs: Vec<(&Shape, Option<u32>)> = leaves_by_layer[li].iter().map(|(s, g)| (s, *g)).collect();
        let mut images = Vec::new();
        // A picture left out for the same reason is said once, not once per sub-layer.
        let mut said: Vec<String> = Vec::new();
        for (si, s) in image_settings {
            let mut notes = Vec::new();
            for mut img in image_inputs(doc, &refs, map, s, &tuning, &mut notes) {
                img.setting = Some(si);
                images.push(img);
            }
            let fresh: Vec<String> = notes.into_iter().filter(|n| !said.contains(n)).collect();
            said.extend(fresh.iter().cloned());
            warnings.extend(fresh);
        }
        entry.images = images;
        // Only the pictures that burn are framed (one left out is not part of the job).
        let mut framed: Vec<&[Point]> = Vec::new();
        for img in &entry.images {
            if framed.contains(&img.outline.as_slice()) {
                continue;
            }
            framed.push(&img.outline);
            let mut p = BezPath::new();
            for (i, pt) in img.outline.iter().enumerate() {
                if i == 0 {
                    p.move_to(*pt);
                } else {
                    p.line_to(*pt);
                }
            }
            if !p.elements().is_empty() {
                frame_paths.push(p);
            }
        }
    }

    // Layers in output order.
    let mut list: Vec<LayerInput> = layers.into_iter().flatten().filter(|l| !l.shapes.is_empty() || !l.images.is_empty()).collect();
    list.sort_by_key(|l| (l.priority, l.index));

    // Bounds of what burns, as the engine anchors the job.
    let job = JobInput { layers: list };
    let bounds = job::generate::job_bounds(&job, input.device.tolerance);
    // The engine anchors on what is left. Once shapes are dropped, it anchors on the bounds the
    // test used instead, so the shapes that are kept burn where they passed it.
    let anchor_bounds = if dropped > 0 && (!st.use_selection_origin || selection_bounds.is_none()) { place_bounds } else { None };
    Ok(Collected { job, bounds, selection_bounds, anchor_bounds, frame_paths, tool_frame, warnings, dropped, size_warnings })
}

/// The origin settings of a build, given the selection bounds.
fn origin_for(input: &BuildInput, selection_bounds: Option<Rect>) -> Result<OriginSettings, BuildError> {
    let st = &input.settings;
    let start_from = match st.start_from {
        StartMode::Absolute => StartFrom::Absolute,
        StartMode::CurrentPosition => StartFrom::CurrentPosition,
        StartMode::UserOrigin => StartFrom::UserOrigin(input.user_origin.ok_or(BuildError::NoUserOrigin)?),
    };
    Ok(OriginSettings {
        machine: input.device.geometry(),
        start_from,
        anchor: st.anchor,
        selection_bounds: if st.use_selection_origin { selection_bounds } else { None },
        finish: st.finish,
    })
}

fn inverse_ops(tp: &Toolpath, t: Affine) -> Toolpath {
    let inv = t.inverse();
    let flip = t.determinant() < 0.0;
    let ops = tp
        .ops
        .iter()
        .map(|op| match op.clone() {
            Op::Rapid { to } => Op::Rapid { to: inv * to },
            Op::Cut { to, speed, power } => Op::Cut { to: inv * to, speed, power },
            Op::Overscan { to, speed } => Op::Overscan { to: inv * to, speed },
            Op::Arc { to, center, clockwise, speed, power } => {
                Op::Arc { to: inv * to, center: inv * center, clockwise: clockwise ^ flip, speed, power }
            }
            other => other,
        })
        .collect();
    Toolpath { space: job::CoordSpace::Design, layers: tp.layers.clone(), ops, start: inv * tp.start }
}

/// Z moves the other way round (DS-04 "Reverse Z").
pub fn reverse_z(mut tp: Toolpath) -> Toolpath {
    for op in &mut tp.ops {
        if let Op::ZMove { z } = op {
            *z = -*z;
        }
    }
    tp
}

/// Fast scan over white space (DS-10): laser-off moves inside fill and image scans run at
/// `speed` (mm/s) instead of the scan speed. Overscan moves keep the scan speed.
pub fn fast_whitespace(tp: &Toolpath, speed: f64) -> Toolpath {
    let mut out = tp.clone();
    let mut raster = false;
    for op in &mut out.ops {
        match op {
            Op::LayerStart { layer } => raster = tp.layers.get(*layer).is_some_and(|l| l.raster),
            Op::Cut { speed: s, power, .. } if raster && *power == 0.0 && speed > *s => *s = speed,
            _ => {}
        }
    }
    out
}

/// True when some operation of `tp` fires the laser (a cut, arc or dwell above power 0).
fn fires_anywhere(tp: &Toolpath) -> bool {
    tp.ops.iter().any(|o| matches!(o, Op::Cut { power, .. } | Op::Arc { power, .. } | Op::Dwell { power, .. } if *power > 0.0))
}

/// Build the job. Runs on any thread; `report` gets 0..1; `cancel` stops it between stages.
pub fn build_job(input: &BuildInput, cancel: Option<&AtomicBool>, report: &mut dyn FnMut(f32)) -> Result<BuiltJob, BuildError> {
    let c = collect(input, cancel, report)?;
    let bounds = c.bounds.ok_or_else(|| c.nothing_to_burn())?;
    let origin = c.origin(input)?;
    let opts = JobOptions { planner: input.settings.planner.clone(), origin: Some(origin), tolerance: input.device.tolerance, comments: true };
    let mut scaled = |f: f32| report(0.3 + 0.7 * f);
    let mut progress = Progress::new(cancel, Some(&mut scaled));
    let toolpath = match job::generate(&c.job, &opts, &mut progress) {
        Ok(tp) => tp,
        Err(JobError::Cancelled) => return Err(BuildError::Cancelled),
        Err(JobError::Empty) => return Err(c.nothing_to_burn()),
        Err(JobError::NotANumber) => return Err(BuildError::NotANumber),
    };
    // A job that fires the laser nowhere (every layer at power 0) is refused like an image at
    // power 0, before anything reaches the machine or a file.
    if !fires_anywhere(&toolpath) {
        return Err(c.nothing_to_burn());
    }
    let design = inverse_ops(&toolpath, origin.transform(bounds));
    let toolpath = if input.device.z_reverse { reverse_z(toolpath) } else { toolpath };
    let mut warnings = c.warnings;
    if c.dropped > 0 {
        warnings.push(tr("job.warn_outside_dropped", "{n} shape(s) outside the work area were left out.").replace("{n}", &c.dropped.to_string()));
    }
    if input.device.no_pauses {
        warnings.extend(crate::machine_job::rd_dropped_pauses(&toolpath));
    }
    let bounds_report = check_work_area(input, &toolpath);
    if !bounds_report.is_ok() {
        let burning = bounds_report.violations.iter().filter(|v| v.burning).count();
        warnings.push(
            tr("job.warn_leaves_work_area", "The job leaves the work area ({n} moves, {burning} of them burning).")
                .replace("{n}", &bounds_report.violations.len().to_string())
                .replace("{burning}", &burning.to_string()),
        );
    }
    if work_area_unchecked(input, &toolpath) {
        warnings.push(work_area_unchecked_note());
    }
    Ok(BuiltJob {
        toolpath,
        design,
        bounds,
        origin,
        warnings,
        bounds_report,
        dropped_shapes: c.dropped,
        fast_whitespace: input.device.fast_whitespace,
    })
}

/// The work-area check in the toolpath's own coordinates; empty when it cannot be checked.
fn check_work_area(input: &BuildInput, tp: &Toolpath) -> BoundsReport {
    let machine_area = input.device.geometry().work_area();
    match tp.space {
        job::CoordSpace::MachineAbsolute => check_bounds(tp, machine_area),
        job::CoordSpace::RelativeToStart => match input.head {
            Some(h) => check_bounds(tp, machine_area.with_origin(Point::new(machine_area.x0 - h.x, machine_area.y0 - h.y))),
            None => BoundsReport::default(),
        },
        job::CoordSpace::Design => check_bounds(tp, input.device.area()),
    }
}

/// True when `check_work_area` cannot check `tp`: it starts from the current position and the
/// head position is unknown (nothing connected).
fn work_area_unchecked(input: &BuildInput, tp: &Toolpath) -> bool {
    matches!(tp.space, job::CoordSpace::RelativeToStart) && input.head.is_none()
}

/// The warning shown when the work area could not be checked.
pub fn work_area_unchecked_note() -> String {
    crate::i18n::tr("job.work_area_unchecked", "The work area could not be checked because the head position is unknown.")
}

/// Refuse a frame that leaves the work area (DS-17 "check bounds when framing").
pub fn check_frame_bounds(input: &BuildInput, frame: &Toolpath) -> Result<(), String> {
    let report = check_work_area(input, frame);
    if report.is_ok() {
        Ok(())
    } else {
        Err(tr("job.frame_leaves_work_area", "The frame leaves the work area ({n} moves). Move the design inside the work area first.")
            .replace("{n}", &report.violations.len().to_string()))
    }
}

/// Build with no progress or cancel (tests, scripts).
pub fn build_job_simple(input: &BuildInput) -> Result<BuiltJob, BuildError> {
    build_job(input, None, &mut |_| {})
}

/// A job being built on a worker thread (PLAN-14: progress and cancel).
pub struct BuildTask {
    rx: Receiver<Result<BuiltJob, BuildError>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    done: Option<Result<Arc<BuiltJob>, BuildError>>,
}

/// Start building on a worker thread.
pub fn spawn_build(input: BuildInput) -> BuildTask {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicU32::new(0));
    let (c2, p2) = (cancel.clone(), progress.clone());
    std::thread::spawn(move || {
        let mut report = |f: f32| p2.store(f.to_bits(), Ordering::Relaxed);
        let r = build_job(&input, Some(&c2), &mut report);
        let _ = tx.send(r);
    });
    BuildTask { rx, cancel, progress, done: None }
}

impl BuildTask {
    /// 0..1.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The result once the worker is done (None while it runs). Safe to call again after.
    pub fn poll(&mut self) -> Option<&Result<Arc<BuiltJob>, BuildError>> {
        if self.done.is_none()
            && let Ok(r) = self.rx.try_recv()
        {
            self.done = Some(r.map(Arc::new));
        }
        self.done.as_ref()
    }

    /// Wait for the result.
    pub fn wait(mut self) -> Result<Arc<BuiltJob>, BuildError> {
        if self.done.is_none() {
            self.done = Some(self.rx.recv().unwrap_or(Err(BuildError::Cancelled)).map(Arc::new));
        }
        self.done.take().unwrap_or(Err(BuildError::Cancelled))
    }
}

#[cfg(test)]
impl BuildTask {
    /// A build for tests that runs until it is cancelled and then takes `lag` longer to stop,
    /// like a worker on a busy computer that notices Cancel late.
    pub fn slow_to_cancel(lag: std::time::Duration) -> BuildTask {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            while !c2.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(lag);
            let _ = tx.send(Err(BuildError::Cancelled));
        });
        BuildTask { rx, cancel, progress: Arc::new(AtomicU32::new(0)), done: None }
    }
}

// ----- G-code ---------------------------------------------------------------------------------

/// The device's own changes to a toolpath before it is written as G-code.
fn for_device(tp: Toolpath, dev: &DeviceGcodeSettings) -> Toolpath {
    if dev.fast_whitespace_scan { fast_whitespace(&tp, dev.fast_whitespace_speed.max(1.0)) } else { tp }
}

/// G-code for the job, or for the job from operation `from_op` on.
pub fn gcode_text(job: &BuiltJob, from_op: Option<usize>, dev: &DeviceGcodeSettings) -> String {
    gcode_text_for(job, from_op, dev, false)
}

/// [`gcode_text`] for a controller that may have no laser mode (`m3_only`, GRBL 1.1e and
/// older): its travel moves switch the laser off themselves.
pub fn gcode_text_for(job: &BuiltJob, from_op: Option<usize>, dev: &DeviceGcodeSettings, m3_only: bool) -> String {
    let mut settings = dev.to_gcode();
    settings.travel_s0 = m3_only;
    match from_op {
        Some(op) => job::gcode::emit(&for_device(job.toolpath_from(op), dev), &settings),
        None => job::gcode::emit(&for_device(job.toolpath.clone(), dev), &settings),
    }
}

/// G-code lines to stream to a controller.
pub fn gcode_lines(tp: &Toolpath, dev: &DeviceGcodeSettings) -> Vec<String> {
    gcode_lines_for(tp, dev, false)
}

/// [`gcode_lines`] for a controller that may have no laser mode (see [`gcode_text_for`]).
pub fn gcode_lines_for(tp: &Toolpath, dev: &DeviceGcodeSettings, m3_only: bool) -> Vec<String> {
    let mut settings = dev.to_gcode();
    settings.travel_s0 = m3_only;
    job::gcode::lines(&for_device(tp.clone(), dev), &settings).collect()
}

/// File extensions the G-code save offers (EXP-06).
pub const GCODE_EXTENSIONS: &[&str] = &["gc", "gcode", "nc", "txt"];

// ----- Framing --------------------------------------------------------------------------------

/// What to frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// The bounding rectangle of the job (LASER-04).
    Rectangle,
    /// The rubber-band outline of the job (LASER-05).
    RubberBand,
    /// The bounding rectangle of the selection.
    Selection,
}

/// The closed rectangle `bounds` as five points.
pub fn frame_rect_points(bounds: Rect) -> Vec<Point> {
    vec![
        Point::new(bounds.x0, bounds.y0),
        Point::new(bounds.x1, bounds.y0),
        Point::new(bounds.x1, bounds.y1),
        Point::new(bounds.x0, bounds.y1),
        Point::new(bounds.x0, bounds.y0),
    ]
}

/// The rubber-band outline of `paths` as a closed run of points.
pub fn frame_rubber_points(paths: &[BezPath], tol: f64) -> Vec<Point> {
    let hull = darkburn_geom::hull::rubber_band_outline(paths, tol);
    let mut pts: Vec<Point> = Vec::new();
    for el in hull.elements() {
        match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => pts.push(*p),
            PathEl::ClosePath => {
                if let Some(first) = pts.first().copied() {
                    pts.push(first);
                }
            }
            PathEl::QuadTo(_, p) => pts.push(*p),
            PathEl::CurveTo(_, _, p) => pts.push(*p),
        }
    }
    if pts.len() > 1 && pts.first() != pts.last() {
        pts.push(pts[0]);
    }
    pts
}

/// A toolpath that walks `points` (design coordinates), laser off unless `power` is given,
/// mapped to the machine the way the job is. The machine side sends this to frame.
pub fn frame_toolpath(points: &[Point], origin: &OriginSettings, job_bounds: Rect, speed: f64, power: Option<f64>) -> Toolpath {
    let mut tp = Toolpath { space: job::CoordSpace::Design, layers: Vec::new(), ops: Vec::new(), start: origin.design_start(job_bounds) };
    if let Some((first, rest)) = points.split_first() {
        tp.ops.push(Op::Rapid { to: *first });
        for p in rest {
            tp.ops.push(Op::Cut { to: *p, speed, power: power.unwrap_or(0.0) });
        }
    }
    let mut o = *origin;
    o.finish = FinishPosition::Stay;
    o.apply(&tp, job_bounds)
}

/// Frame toolpath for the current document and settings.
pub fn build_frame(input: &BuildInput, kind: FrameKind, speed: f64, power: Option<f64>) -> Result<Toolpath, BuildError> {
    let c = collect(input, None, &mut |_| {})?;
    let bounds = c.bounds.ok_or_else(|| c.nothing_to_burn())?;
    let origin = c.origin(input)?;
    let points = match kind {
        FrameKind::Rectangle => {
            let mut r = bounds;
            for p in &c.tool_frame {
                r = r.union(darkburn_core::kurbo::Shape::bounding_box(p));
            }
            frame_rect_points(r)
        }
        FrameKind::RubberBand => {
            let all: Vec<BezPath> = c.frame_paths.iter().chain(&c.tool_frame).cloned().collect();
            frame_rubber_points(&all, input.device.tolerance)
        }
        FrameKind::Selection => {
            if input.selection.is_empty() {
                return Err(BuildError::NothingSelected);
            }
            frame_rect_points(c.selection_bounds.ok_or(BuildError::NothingSelected)?)
        }
    };
    let tp = frame_toolpath(&points, &origin, bounds, speed, power);
    // Framing with the red dot (laser off): move the head so the dot, not the laser, lands on the frame.
    Ok(match (power, input.device.pointer_offset) {
        (None, [dx, dy]) if dx != 0.0 || dy != 0.0 => job::origin::pointer_offset(&tp, darkburn_core::kurbo::Vec2::new(-dx, -dy)),
        _ => tp,
    })
}

/// Summary numbers of a simulated job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct JobStats {
    pub seconds: f64,
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub scan_mm: f64,
    pub ops: usize,
}

impl JobStats {
    pub fn of(job: &BuiltJob, sim: &job::Simulation) -> JobStats {
        JobStats {
            seconds: sim.total_time,
            cut_mm: sim.cut_distance,
            travel_mm: sim.rapid_distance,
            scan_mm: sim.scan_distance,
            ops: job.toolpath.ops.len(),
        }
    }
}

/// "1h 02m 03s" style time.
pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}h {:02}m {:02}s", s / 3600, (s / 60) % 60, s % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Rect;

    fn input(doc: Document) -> BuildInput {
        BuildInput {
            doc,
            selection: Vec::new(),
            settings: JobProjectSettings::default(),
            device: JobDevice::new(200.0, 200.0),
            user_origin: None,
            head: None,
            motion: AdditionalSettings::default(),
            tuning: None,
        }
    }

    fn line_layer(doc: &mut Document, l: usize, speed: f64, power: f64) {
        let s = doc.layer_mut(l).main_mut();
        s.mode = Mode::Line;
        s.speed = speed;
        s.max_power = power;
    }

    fn square_doc() -> Document {
        let mut d = Document::new();
        line_layer(&mut d, 0, 20.0, 50.0);
        d.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
        d
    }

    #[test]
    fn square_makes_a_square_toolpath_and_gcode() {
        let b = build_job_simple(&input(square_doc())).unwrap();
        assert!((b.bounds.width() - 20.0).abs() < 1e-6 && (b.bounds.x0 - 10.0).abs() < 1e-6);
        let burns = b.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power > 0.0)).count();
        assert_eq!(burns, 4);
        let g = gcode_text(&b, None, &DeviceGcodeSettings::default());
        assert!(g.contains("G21") || g.contains("G1"), "{g}");
        assert!(g.contains("S500"), "50 % of 1000: {g}");
        assert!(g.contains("F1200"), "20 mm/s is 1200 mm/min: {g}");
        assert!(b.bounds_report.is_ok());
        // The design toolpath has the same shape at the same indices.
        assert_eq!(b.design.ops.len(), b.toolpath.ops.len());
    }

    #[test]
    fn origin_corner_flips_machine_coordinates_but_not_the_preview() {
        let mut i = input(square_doc());
        i.device.origin = Corner::BottomRight;
        let b = build_job_simple(&i).unwrap();
        let (m, d): (Vec<Point>, Vec<Point>) =
            (b.toolpath.ops.iter().filter_map(Op::target).collect(), b.design.ops.iter().filter_map(Op::target).collect());
        assert!(m.iter().all(|p| p.x >= 200.0 - 30.0 - 1e-6 && p.x <= 200.0 - 10.0 + 1e-6), "machine x is mirrored: {m:?}");
        assert!(d.iter().all(|p| p.x >= 10.0 - 1e-6 && p.x <= 30.0 + 1e-6), "design x is as drawn: {d:?}");
    }

    #[test]
    fn current_position_is_relative_and_anchored() {
        let mut i = input(square_doc());
        i.settings.start_from = StartMode::CurrentPosition;
        i.settings.anchor = JobAnchor::Center;
        let b = build_job_simple(&i).unwrap();
        assert_eq!(b.toolpath.space, job::CoordSpace::RelativeToStart);
        let pts: Vec<Point> = b.toolpath.ops.iter().filter_map(Op::target).collect();
        assert!(pts.iter().all(|p| p.x.abs() <= 10.0 + 1e-6 && p.y.abs() <= 10.0 + 1e-6), "{pts:?}");
        // No head position known: the work area cannot be checked.
        assert!(b.bounds_report.is_ok());
    }

    #[test]
    fn user_origin_needs_one_and_places_the_anchor_there() {
        let mut i = input(square_doc());
        i.settings.start_from = StartMode::UserOrigin;
        assert_eq!(build_job_simple(&i).unwrap_err(), BuildError::NoUserOrigin);
        i.user_origin = Some(Point::new(100.0, 100.0));
        let b = build_job_simple(&i).unwrap();
        let r = b.toolpath.ops.iter().filter_map(Op::target).fold(None::<Rect>, |r, p| Some(r.map_or(Rect::from_points(p, p), |r| r.union_pt(p))));
        let r = r.unwrap();
        assert!((r.x0 - 100.0).abs() < 1e-6 && (r.y0 - 100.0).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn use_selection_origin_anchors_on_the_selection() {
        let mut d = square_doc();
        let other = d.add(Shape::rect(Rect::new(100.0, 100.0, 120.0, 120.0), 0.0, 0));
        let mut i = input(d);
        i.selection = vec![other];
        i.settings.start_from = StartMode::UserOrigin;
        i.user_origin = Some(Point::new(0.0, 0.0));
        i.settings.use_selection_origin = true;
        let b = build_job_simple(&i).unwrap();
        // The selection's bottom-left lands on the user origin, the other square sits before it.
        let r = b
            .toolpath
            .ops
            .iter()
            .filter_map(Op::target)
            .fold(None::<Rect>, |r, p| Some(r.map_or(Rect::from_points(p, p), |r| r.union_pt(p))))
            .unwrap();
        assert!((r.x1 - 20.0).abs() < 1e-6 && (r.y1 - 20.0).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn cut_selected_only_burns_the_selection() {
        let mut d = square_doc();
        let b2 = d.add(Shape::rect(Rect::new(100.0, 100.0, 110.0, 110.0), 0.0, 0));
        let mut i = input(d);
        i.settings.cut_selected = true;
        assert_eq!(build_job_simple(&i).unwrap_err(), BuildError::NothingSelected);
        i.selection = vec![b2];
        let b = build_job_simple(&i).unwrap();
        assert!((b.bounds.x0 - 100.0).abs() < 1e-6 && (b.bounds.width() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn circle_with_tabs_leaves_gaps() {
        let mut d = Document::new();
        line_layer(&mut d, 1, 10.0, 80.0);
        {
            let s = d.layer_mut(1).main_mut();
            s.tabs.enabled = true;
            s.tabs.auto_spacing = true;
            s.tabs.spacing = 30.0;
            s.tabs.size = 2.0;
            s.tabs.min_count = 4;
            s.tabs.max_count = 4;
        }
        d.add(Shape::ellipse(Rect::new(20.0, 20.0, 60.0, 60.0), 1));
        let b = build_job_simple(&input(d)).unwrap();
        let off: f64 = b.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } | Op::Arc { power, .. } if *power == 0.0)).count() as f64;
        assert!(off >= 3.0, "tabs make laser-off moves: {off}");
        let sim = b.simulate(&AdditionalSettings::default());
        assert!(sim.total_time > 0.0 && sim.scan_distance > 0.0, "{sim:?}");
    }

    #[test]
    fn manual_tabs_reach_the_job() {
        let off = |tabs: Vec<[f64; 2]>| {
            let mut d = Document::new();
            line_layer(&mut d, 1, 10.0, 80.0);
            {
                let s = d.layer_mut(1).main_mut();
                s.tabs.enabled = true;
                s.tabs.auto_spacing = false;
                s.tabs.size = 2.0;
            }
            let mut sh = Shape::rect(Rect::new(10.0, 10.0, 50.0, 50.0), 0.0, 1);
            sh.tabs = tabs;
            d.add(sh);
            let b = build_job_simple(&input(d)).unwrap();
            b.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power == 0.0)).count()
        };
        assert_eq!(off(vec![]), 0, "no automatic tabs and no manual ones: solid cut");
        assert!(off(vec![[0.0, -20.0]]) >= 1, "a manual tab leaves a gap");
        assert!(off(vec![[0.0, -20.0], [20.0, 0.0]]) >= 2);
    }

    #[test]
    fn forced_start_point_and_direction_reach_the_job() {
        let first_move = |start: Option<[f64; 2]>, dir: darkburn_core::CutDirection| {
            let mut d = Document::new();
            line_layer(&mut d, 1, 10.0, 80.0);
            let mut sh = Shape::rect(Rect::new(10.0, 10.0, 50.0, 50.0), 0.0, 1);
            sh.start_point = start;
            sh.direction = dir;
            d.add(sh);
            let b = build_job_simple(&input(d)).unwrap();
            let pts: Vec<Point> = b.toolpath.ops.iter().filter_map(Op::target).collect();
            (pts[0], pts[1])
        };
        let (a, _) = first_move(Some([20.0, 0.0]), darkburn_core::CutDirection::Auto);
        assert!(a.distance(Point::new(50.0, 30.0)) < 1e-6, "starts at the forced point: {a:?}");
        let (_, cw1) = first_move(Some([0.0, -20.0]), darkburn_core::CutDirection::Clockwise);
        let (_, ccw1) = first_move(Some([0.0, -20.0]), darkburn_core::CutDirection::CounterClockwise);
        assert!(cw1.x < 30.0 && ccw1.x > 30.0, "directions differ: cw {cw1:?} ccw {ccw1:?}");
    }

    #[test]
    fn fill_layer_scans_lines() {
        let mut d = Document::new();
        {
            let s = d.layer_mut(2).main_mut();
            s.mode = Mode::Fill;
            s.interval = 1.0;
            s.speed = 100.0;
            s.max_power = 30.0;
        }
        d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 2));
        let b = build_job_simple(&input(d)).unwrap();
        let burns = b.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power > 0.0)).count();
        assert!((9..=11).contains(&burns), "about 10 scan lines: {burns}");
    }

    #[test]
    fn a_fill_far_too_big_to_scan_is_left_out_with_a_warning() {
        // Found by the monkey (seed 2006): shapes scaled up by accident to thousands of
        // kilometres on a fill layer took all the memory there is and the program with it.
        let mut d = Document::new();
        d.layer_mut(2).main_mut().mode = Mode::Fill;
        d.layer_mut(2).main_mut().interval = 0.1;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 6e7, 6e7), 0.0, 2));
        d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        let b = build_job_simple(&input(d.clone())).unwrap();
        assert!(b.warnings.iter().any(|w| w.contains("left out") && w.contains("scan lines")), "{:?}", b.warnings);
        assert!(b.toolpath.ops.len() < 100, "only the small square is cut: {} operations", b.toolpath.ops.len());
        // A fill that fits a large machine is not touched by the limit.
        d.layer_mut(2).main_mut().interval = 1.0;
        let s = d.iter().next().unwrap().id;
        d.get_mut(s).unwrap().kind = ShapeKind::Rect { width: 3000.0, height: 1500.0, radius: 0.0 };
        let b = build_job_simple(&input(d)).unwrap();
        assert!(!b.warnings.iter().any(|w| w.contains("scan lines")), "{:?}", b.warnings);
        assert!(b.toolpath.ops.len() > 1500);
    }

    #[test]
    fn dots_or_perforation_far_too_many_are_left_out_with_a_warning() {
        // An outline of about 8e9 mm split into dots or perforation took all the memory there
        // is; the layer is now left out as a fill that size is, and other layers are unchanged.
        let set = |d: &mut Document, l: usize, dots: bool| {
            let s = d.layer_mut(l).main_mut();
            s.mode = Mode::Line;
            if dots {
                s.dot_mode = true;
                s.dot_spacing = 0.1;
            } else {
                s.perforation = true;
                s.perf_cut = 1.0;
                s.perf_skip = 1.0;
            }
        };
        for dots in [true, false] {
            let mut small = Document::new();
            set(&mut small, 0, dots);
            small.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
            let plain = build_job_simple(&input(small.clone())).unwrap();
            assert!(!plain.warnings.iter().any(|w| w.contains("dots or perforation")), "{:?}", plain.warnings);
            let pieces = if dots {
                plain.toolpath.ops.iter().filter(|o| matches!(o, Op::Dwell { .. })).count()
            } else {
                plain.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power == 0.0)).count()
            };
            assert!(pieces >= 20, "a normal outline still gets its pieces: {pieces}");
            let mut big = small.clone();
            set(&mut big, 2, dots);
            big.add(Shape::rect(Rect::new(0.0, 0.0, 2e9, 2e9), 0.0, 2));
            let started = std::time::Instant::now();
            let b = build_job_simple(&input(big)).unwrap();
            assert!(started.elapsed().as_secs() < 30, "held by the limit before any piece is made");
            assert!(b.warnings.iter().any(|w| w.contains("left out") && w.contains("dots or perforation")), "{:?}", b.warnings);
            assert_eq!(b.toolpath.ops, plain.toolpath.ops, "only the small square is cut (dots {dots})");
        }
    }

    #[test]
    fn far_too_many_pieces_counts_zero_passes_every_setting_every_pass_and_the_kerf() {
        // The estimate follows what the generator makes: a setting at 0 passes still cuts once,
        // each splitting setting and each pass make their own pieces, and a kerf cuts a longer
        // outline. Each layer below is left out, with a small square on layer 0 kept.
        let left_out = |d: Document, why: &str| {
            let mut d = d;
            d.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
            let b = build_job_simple(&input(d)).unwrap();
            assert!(b.warnings.iter().any(|w| w.contains("left out") && w.contains("dots or perforation")), "{why}: {:?}", b.warnings);
            assert!(b.toolpath.ops.len() < 100, "{why}: only the small square is cut ({} operations)", b.toolpath.ops.len());
        };
        let perforated = |d: &mut Document, passes: u32| {
            let s = d.layer_mut(2).main_mut();
            s.mode = Mode::Line;
            s.perforation = true;
            s.perf_cut = 0.5;
            s.perf_skip = 0.5;
            s.passes = passes;
        };
        // Dots at 0 passes on a huge outline.
        let mut d = Document::new();
        let s = d.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 0.1;
        s.passes = 0;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 2e9, 2e9), 0.0, 2));
        left_out(d, "zero passes");
        // 120 000 mm of outline at a 1 mm step is 120 000 pieces a pass: 50 passes are 6 million.
        let big = || Shape::rect(Rect::new(0.0, 0.0, 30_000.0, 30_000.0), 0.0, 2);
        let mut d = Document::new();
        perforated(&mut d, 50);
        d.add(big());
        left_out(d, "many passes");
        // Two settings of 25 passes each: 3 million pieces apiece, 6 million together.
        let mut d = Document::new();
        perforated(&mut d, 25);
        let second = d.layer(2).main().clone();
        d.layer_mut(2).settings.push(second);
        d.add(big());
        left_out(d, "two settings");
        // A 20 mm square with a 10 m kerf: 8000 dots as drawn, about 6.3 million cut.
        let mut d = Document::new();
        let s = d.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 0.01;
        s.kerf = 10_000.0;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 20.0), 0.0, 2));
        left_out(d, "big kerf");
    }

    /// Builds `d` with a small square on layer 0 and checks that layer 2 is left out whole
    /// with the dots-or-perforation warning, and that only the square is cut.
    fn far_too_many_left_out(mut d: Document, why: &str) {
        d.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
        let started = std::time::Instant::now();
        let b = build_job_simple(&input(d)).unwrap();
        assert!(started.elapsed().as_secs() < 30, "{why}: held before the pieces are made");
        let said: Vec<&String> = b.warnings.iter().filter(|w| w.contains("left out") && w.contains("dots or perforation")).collect();
        assert!(said.len() == 1 && said[0].contains(&layer::layer_label(2)), "{why}: {:?}", b.warnings);
        assert!(b.toolpath.ops.len() < 100, "{why}: only the small square is cut ({} operations)", b.toolpath.ops.len());
        assert!(!b.toolpath.ops.iter().any(|o| matches!(o, Op::Dwell { .. })), "{why}: no dot of layer 2 fires");
        assert!(b.toolpath.layers.iter().all(|l| l.layer_index == 0), "{why}: nothing of layer 2 is left");
    }

    #[test]
    fn far_too_many_pieces_counts_the_overcut_of_perforation() {
        // A 2 mm square perforated every 0.02 mm, 10 000 passes: 4 million gaps as drawn, but
        // an 8 mm overcut doubles the path that is perforated.
        let mut d = Document::new();
        let s = d.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.perforation = true;
        s.perf_cut = 0.01;
        s.perf_skip = 0.01;
        s.passes = 10_000;
        s.overcut = 8.0;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 2.0, 2.0), 0.0, 2));
        far_too_many_left_out(d, "overcut");
    }

    #[test]
    fn far_too_many_pieces_counts_one_dot_per_outline_per_pass() {
        // A 1 mm square with dots 100 mm apart still gets its first dot on every pass:
        // 20 million passes are 20 million dots.
        let mut d = Document::new();
        let s = d.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 100.0;
        s.passes = 20_000_000;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 2));
        far_too_many_left_out(d, "short outline");
    }

    #[test]
    fn far_too_many_pieces_are_counted_on_the_real_outline_after_a_stretch() {
        // A print-and-cut fit that stretches X 1000 times: the estimate scales the kerf corners
        // by the average stretch (about 0.4 million dots), but the corners become ellipses
        // 20 m long and the real outline is about 8 million dots. The real count leaves the
        // layer out, whole.
        use darkburn_vision::printcut::{PnCFit, PnCMode, ResidualReport};
        let mut d = Document::new();
        let s = d.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 0.01;
        s.kerf = 20.0;
        d.add(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1000.0), 0.0, 2));
        let mut v = crate::vision_state::VisionState::default();
        v.pnc.fit = Some(PnCFit {
            mode: PnCMode::FullAffine,
            affine: Affine::scale_non_uniform(1000.0, 1.0),
            scale: 1000f64.sqrt(),
            rotation_deg: 0.0,
            translation: (0.0, 0.0),
            residuals: ResidualReport { per_target: Vec::new(), rms_mm: 0.0, max_mm: 0.0 },
        });
        d.vision_settings = Some(v.to_value());
        far_too_many_left_out(d, "stretched kerf corners");
    }

    #[test]
    fn far_too_many_real_pieces_left_out_do_not_move_the_other_layers() {
        // Layer 2 passes the early estimate, but its real count (kerf corners stretched 1000
        // times in X) is past the limit. With "Remove what is outside the work area" from a
        // user origin, it must not take part in placing the job: layer 0 burns where it does
        // without layer 2, instead of being pushed off the bed by it and discarded.
        use darkburn_vision::printcut::{PnCFit, PnCMode, ResidualReport};
        let mut v = crate::vision_state::VisionState::default();
        v.pnc.fit = Some(PnCFit {
            mode: PnCMode::FullAffine,
            affine: Affine::scale_non_uniform(1000.0, 1.0),
            scale: 1000f64.sqrt(),
            rotation_deg: 0.0,
            translation: (0.0, 0.0),
            residuals: ResidualReport { per_target: Vec::new(), rms_mm: 0.0, max_mm: 0.0 },
        });
        let mut alone = Document::new();
        alone.vision_settings = Some(v.to_value());
        // Mapped to [-200, -190] x [0, 10].
        alone.add(Shape::rect(Rect::new(-0.2, 0.0, -0.19, 10.0), 0.0, 0));
        let mut both = alone.clone();
        let s = both.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 0.01;
        s.kerf = 20.0;
        both.add(Shape::rect(Rect::new(0.0, 0.0, 0.001, 100.0), 0.0, 2));
        let build = |d: Document| {
            let mut i = input(d);
            i.settings.discard_out_of_bounds = true;
            i.settings.start_from = StartMode::UserOrigin;
            i.settings.anchor = JobAnchor::Center;
            i.user_origin = Some(Point::new(99.5, 100.0));
            build_job_simple(&i).unwrap()
        };
        let plain = build(alone);
        assert!(plain.toolpath.ops.iter().any(|o| matches!(o, Op::Cut { power, .. } if *power > 0.0)), "layer 0 burns alone");
        let b = build(both);
        assert!(
            b.warnings.iter().any(|w| w.contains("left out") && w.contains("dots or perforation") && w.contains(&layer::layer_label(2))),
            "{:?}",
            b.warnings
        );
        assert!(b.toolpath.layers.iter().all(|l| l.layer_index == 0), "nothing of layer 2 is left");
        assert_eq!(b.toolpath.ops, plain.toolpath.ops, "layer 0 burns where it does without layer 2");
    }

    #[test]
    fn far_too_many_real_pieces_leave_the_whole_layer_out() {
        // The guard on the real count, called on its own: a layer past the limit loses every
        // shape (nothing of it is generated), one within it is untouched.
        let layer = |passes: u32| {
            let mut d = Document::new();
            let s = d.layer_mut(2).main_mut();
            s.mode = Mode::Line;
            s.dot_mode = true;
            s.dot_spacing = 1.0;
            for x in [0.0, 20.0, 40.0] {
                d.add(Shape::rect(Rect::new(x, 0.0, x + 10.0, 10.0), 0.0, 2));
            }
            // Built at one pass (the early estimate lets it through), then given its passes.
            let mut l = collect(&input(d), None, &mut |_| {}).unwrap().job.layers.remove(0);
            l.settings[0].passes = passes;
            l
        };
        // 3 squares of 40 dots (and the first again): 123 a pass.
        let mut layers: Vec<Option<LayerInput>> = (0..layer::LAYER_COUNT).map(|_| None).collect();
        layers[2] = Some(layer(1_000));
        assert!(drop_too_many_pieces(&mut layers, 0.01, &[0.0; layer::LAYER_COUNT]).is_empty());
        assert_eq!(layers[2].as_ref().unwrap().shapes.len(), 3);
        layers[2] = Some(layer(100_000));
        let n = job::line_pieces(layers[2].as_ref().unwrap(), 0.01, f64::INFINITY);
        assert!((n - 12_300_000.0).abs() < 1.0, "{n}");
        // Counting stops at the second square, once past the limit.
        let held = job::line_pieces(layers[2].as_ref().unwrap(), 0.01, MAX_LINE_PIECES);
        assert!((held - 8_200_000.0).abs() < 1.0, "{held}");
        let dropped = drop_too_many_pieces(&mut layers, 0.01, &[0.0; layer::LAYER_COUNT]);
        assert!(dropped.len() == 1 && dropped[0].0 == 2 && dropped[0].1.contains("dots or perforation"), "{dropped:?}");
        assert!(layers[2].as_ref().unwrap().shapes.is_empty(), "the whole layer is left out");
    }

    #[test]
    fn far_too_big_alone_gives_the_reason_not_just_nothing_to_burn() {
        // A document whose only shapes are left out for their size says why.
        let mut dots = Document::new();
        let s = dots.layer_mut(2).main_mut();
        s.mode = Mode::Line;
        s.dot_mode = true;
        s.dot_spacing = 0.1;
        dots.add(Shape::rect(Rect::new(0.0, 0.0, 2e9, 2e9), 0.0, 2));
        let mut fill = Document::new();
        fill.layer_mut(2).main_mut().mode = Mode::Fill;
        fill.layer_mut(2).main_mut().interval = 0.1;
        fill.add(Shape::rect(Rect::new(0.0, 0.0, 6e7, 6e7), 0.0, 2));
        for (d, what) in [(dots, "dots or perforation"), (fill, "scan lines")] {
            let job = build_job_simple(&input(d.clone())).unwrap_err();
            let BuildError::TooBig(reasons) = &job else { panic!("{what}: {job:?}") };
            assert!(reasons.len() == 1 && reasons[0].contains(what), "{reasons:?}");
            assert!(job.to_string().contains(what) && job.to_string().contains("nothing left to burn"), "{job}");
            let frame = build_frame(&input(d), FrameKind::Rectangle, 10.0, None).unwrap_err();
            assert_eq!(frame, job, "the frame says the same");
        }
        // An ordinarily empty document stays a plain "nothing to burn".
        assert_eq!(build_job_simple(&input(Document::new())).unwrap_err(), BuildError::NothingToBurn);
    }

    #[test]
    fn kerf_grows_closed_shapes() {
        let mut d = square_doc();
        d.layer_mut(0).main_mut().kerf = 1.0;
        let b = build_job_simple(&input(d)).unwrap();
        assert!((b.bounds.width() - 22.0).abs() < 0.05, "{:?}", b.bounds);
    }

    #[test]
    fn a_hole_the_kerf_closes_up_is_said() {
        use darkburn_core::kurbo::Shape as _;
        let mut d = square_doc();
        d.layer_mut(0).main_mut().kerf = 0.5;
        // a 0.4 mm hole in the square, and one that is big enough
        d.add(Shape::path(darkburn_core::kurbo::Circle::new((15.0, 15.0), 0.2).to_path(0.001), 0));
        d.add(Shape::path(darkburn_core::kurbo::Circle::new((25.0, 25.0), 3.0).to_path(0.001), 0));
        let b = build_job_simple(&input(d)).unwrap();
        let said: Vec<&String> = b.warnings.iter().filter(|w| w.contains("kerf")).collect();
        assert_eq!(said.len(), 1, "{:?}", b.warnings);
        assert!(said[0].contains("1 shape or hole") && said[0].contains("left out"), "{}", said[0]);
        // without a kerf nothing is said
        let mut d = square_doc();
        d.add(Shape::path(darkburn_core::kurbo::Circle::new((15.0, 15.0), 0.2).to_path(0.001), 0));
        assert!(build_job_simple(&input(d)).unwrap().warnings.iter().all(|w| !w.contains("kerf")));
    }

    #[test]
    fn image_layer_makes_scanlines() {
        let mut d = Document::new();
        {
            let s = d.layer_mut(3).main_mut();
            s.mode = Mode::Image;
            s.interval = 0.5;
            s.image_mode = darkburn_core::layer::ImageMode::Threshold;
            s.speed = 200.0;
        }
        // A 4 x 4 pixel black PNG placed as 10 x 10 mm at (5, 5) to (15, 15).
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 0]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let key = d.add_image(darkburn_core::ImageData { format: "png".into(), bytes: Arc::from(png), px_width: 4, px_height: 4 });
        let shape = Shape::new(
            ShapeKind::Image(ImageShape { image: key, width: 10.0, height: 10.0, ..Default::default() }),
            Affine::translate((10.0, 10.0)),
            3,
        );
        d.add(shape);
        let b = build_job_simple(&input(d)).unwrap();
        assert!((b.bounds.x0 - 5.0).abs() < 0.6 && (b.bounds.x1 - 15.0).abs() < 0.6, "{:?}", b.bounds);
        let burns = b.toolpath.ops.iter().filter(|o| matches!(o, Op::Cut { power, .. } if *power > 0.0)).count();
        assert!(burns >= 15, "20 scan lines of 10 mm: {burns}");
    }

    /// A 10 x 10 mm black picture on layer 3, whose main setting is Image at 0.5 mm, 200 mm/s,
    /// 60 %, with a second Image sub-layer at 1 mm, 100 mm/s, 30 %.
    fn two_image_sub_layers() -> Document {
        let mut d = Document::new();
        {
            let l = d.layer_mut(3);
            let s = l.main_mut();
            s.mode = Mode::Image;
            s.interval = 0.5;
            s.image_mode = darkburn_core::layer::ImageMode::Threshold;
            s.speed = 200.0;
            s.max_power = 60.0;
            let mut sub = s.clone();
            sub.interval = 1.0;
            sub.speed = 100.0;
            sub.max_power = 30.0;
            l.settings.push(sub);
        }
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 0]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let key = d.add_image(darkburn_core::ImageData { format: "png".into(), bytes: Arc::from(png), px_width: 4, px_height: 4 });
        d.add(Shape::new(
            ShapeKind::Image(ImageShape { image: key, width: 10.0, height: 10.0, ..Default::default() }),
            Affine::translate((10.0, 10.0)),
            3,
        ));
        d
    }

    /// Per sub-layer that burns, in job order: (sub-layer, burning scan lines, their speeds,
    /// highest power).
    fn image_passes(b: &BuiltJob) -> Vec<(usize, usize, Vec<f64>, f64)> {
        let mut out: Vec<(usize, usize, Vec<f64>, f64)> = Vec::new();
        for op in &b.toolpath.ops {
            match op {
                Op::LayerStart { layer } => out.push((b.toolpath.layers[*layer].sublayer, 0, Vec::new(), 0.0)),
                Op::Cut { speed, power, .. } if *power > 0.0 => {
                    let last = out.last_mut().expect("a cut inside a layer");
                    last.1 += 1;
                    if !last.2.contains(speed) {
                        last.2.push(*speed);
                    }
                    last.3 = last.3.max(*power);
                }
                _ => {}
            }
        }
        out.retain(|p| p.1 > 0);
        out
    }

    #[test]
    fn two_image_sub_layers_each_scan_with_their_own_settings() {
        let b = build_job_simple(&input(two_image_sub_layers())).unwrap();
        let passes = image_passes(&b);
        assert_eq!(passes.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 1], "one image pass per sub-layer, in order: {passes:?}");
        let (fine, coarse) = (&passes[0], &passes[1]);
        // 10 mm at 0.5 mm is 20 lines, at 1 mm 10 lines: the second sub-layer has its own interval.
        assert!(fine.1 >= 15 && fine.1 <= 22, "0.5 mm sub-layer: {fine:?}");
        assert!(coarse.1 >= 7 && coarse.1 <= 12, "1 mm sub-layer: {coarse:?}");
        assert_eq!(fine.2, vec![200.0], "{fine:?}");
        assert_eq!(coarse.2, vec![100.0], "{coarse:?}");
        assert!((fine.3 / coarse.3 - 2.0).abs() < 1e-6, "60 % against 30 %: {passes:?}");
        // The picture is framed once.
        let c = collect(&input(two_image_sub_layers()), None, &mut |_| {}).unwrap();
        assert_eq!(c.frame_paths.len(), 1);
    }

    #[test]
    fn a_disabled_image_sub_layer_does_not_burn() {
        let mut d = two_image_sub_layers();
        d.layer_mut(3).settings[0].enabled = false;
        let b = build_job_simple(&input(d)).unwrap();
        let passes = image_passes(&b);
        assert_eq!(passes.len(), 1, "{passes:?}");
        assert_eq!(passes[0].0, 1);
        assert!(passes[0].1 >= 7 && passes[0].1 <= 12, "only the 1 mm sub-layer: {passes:?}");
        assert_eq!(passes[0].2, vec![100.0]);
    }

    #[test]
    fn out_of_bounds_is_reported_and_can_be_dropped() {
        let mut d = square_doc();
        d.add(Shape::rect(Rect::new(190.0, 190.0, 230.0, 230.0), 0.0, 0));
        let mut i = input(d);
        let b = build_job_simple(&i).unwrap();
        assert!(b.out_of_bounds());
        assert!(b.warnings.iter().any(|w| w.contains("work area")));
        i.settings.discard_out_of_bounds = true;
        let b = build_job_simple(&i).unwrap();
        assert!(!b.out_of_bounds());
        assert_eq!(b.dropped_shapes, 1);
    }

    #[test]
    fn start_here_slices_the_job() {
        let mut d = square_doc();
        d.add(Shape::rect(Rect::new(50.0, 50.0, 60.0, 60.0), 0.0, 0));
        let b = build_job_simple(&input(d)).unwrap();
        let total = b.toolpath.ops.len();
        let sliced = b.toolpath_from(total / 2);
        assert!(sliced.ops.len() < total + 6);
        let text = gcode_text(&b, Some(total / 2), &DeviceGcodeSettings::default());
        assert!(!text.is_empty());
    }

    #[test]
    fn optimization_settings_reach_the_planner_and_the_project() {
        let mut d = Document::new();
        line_layer(&mut d, 0, 20.0, 10.0);
        for i in 0..4 {
            d.add(Shape::rect(Rect::new(i as f64 * 30.0, 0.0, i as f64 * 30.0 + 10.0, 10.0), 0.0, 0));
        }
        let mut st = JobProjectSettings::default();
        st.planner.optimize = false;
        st.store(&mut d);
        let back = JobProjectSettings::from_doc(&d, &PlannerSettings::default());
        assert!(!back.planner.optimize);
        assert_eq!(back, st);
        let mut i = input(d);
        i.settings = back;
        assert!(build_job_simple(&i).is_ok());
        // A project without saved settings takes the user's defaults.
        let fallback = PlannerSettings { inner_first: false, ..Default::default() };
        assert!(!JobProjectSettings::from_doc(&Document::new(), &fallback).planner.inner_first);
    }

    #[test]
    fn cancel_and_progress_work_off_the_ui_thread() {
        let task = spawn_build(input(square_doc()));
        let r = task.wait().unwrap();
        assert!(!r.toolpath.ops.is_empty());
        let t = spawn_build(input(square_doc()));
        t.cancel();
        // Either it was already done or it reports the cancel; it never hangs.
        let _ = t.wait();
        let mut empty = input(Document::new());
        empty.settings.cut_selected = false;
        assert_eq!(build_job_simple(&empty).unwrap_err(), BuildError::NothingToBurn);
    }

    #[test]
    fn frames_follow_the_job() {
        let i = input(square_doc());
        let tp = build_frame(&i, FrameKind::Rectangle, 100.0, None).unwrap();
        assert_eq!(tp.ops.len(), 5);
        assert!(matches!(tp.ops[0], Op::Rapid { .. }));
        let rb = build_frame(&i, FrameKind::RubberBand, 100.0, Some(2.0)).unwrap();
        assert!(rb.ops.len() >= 4);
        assert!(rb.ops.iter().any(|o| matches!(o, Op::Cut { power, .. } if *power == 2.0)));
        assert!(build_frame(&i, FrameKind::Selection, 100.0, None).is_err());
        assert_eq!(frame_rect_points(Rect::new(0.0, 0.0, 2.0, 1.0)).len(), 5);
    }

    #[test]
    fn gcode_settings_scripts_and_flavor() {
        let mut g = DeviceGcodeSettings { start_script: "G54".into(), end_script: "M5".into(), auto_home: true, ..Default::default() };
        let b = build_job_simple(&input(square_doc())).unwrap();
        let text = gcode_text(&b, None, &g);
        assert!(text.contains("$H") && text.contains("G54"), "{text}");
        g.set_flavor_index(3);
        assert_eq!(g.s_max, 255.0);
        assert_eq!(format_time(3723.0), "1h 02m 03s");
    }
}
