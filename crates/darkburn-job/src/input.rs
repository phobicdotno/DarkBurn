//! Input types: what the app hands the job engine.
//!
//! Everything is in millimetres, Y up, in design (bed) coordinates with the origin at the
//! bottom-left of the work area. Powers are percent (0-100) of the controller's maximum.
//! Speeds are mm/s.

use kurbo::{Affine, BezPath, Point};
use serde::{Deserialize, Serialize};

/// A whole job: the layers to burn.
#[derive(Clone, Debug, Default)]
pub struct JobInput {
    pub layers: Vec<LayerInput>,
}

/// One colour layer with its cut settings and shapes.
#[derive(Clone, Debug)]
pub struct LayerInput {
    /// Colour index (C00 = 0 ...), used in comments and as a stable id.
    pub index: usize,
    pub name: String,
    /// Global layer priority: lower burns first. Ties keep the order in `JobInput::layers`.
    pub priority: i32,
    /// Output on or off. Layers with output off are skipped.
    pub output: bool,
    /// Main settings first, then sub-layers in the order they run.
    pub settings: Vec<CutSettings>,
    pub shapes: Vec<ShapeInput>,
    pub images: Vec<ImageInput>,
}

impl LayerInput {
    pub fn new(index: usize, name: impl Into<String>, settings: CutSettings) -> Self {
        Self { index, name: name.into(), priority: index as i32, output: true, settings: vec![settings], shapes: Vec::new(), images: Vec::new() }
    }

    pub fn with_shape(mut self, shape: ShapeInput) -> Self {
        self.shapes.push(shape);
        self
    }
}

/// Direction the user forced on a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForcedDirection {
    /// Keep the direction the path was drawn in.
    AsDrawn,
    /// Reverse the drawn direction.
    Reversed,
    /// Closed shapes: clockwise. Open shapes keep their drawn direction.
    Clockwise,
    /// Closed shapes: counter-clockwise. Open shapes keep their drawn direction.
    CounterClockwise,
}

/// A vector shape on a layer.
#[derive(Clone, Debug)]
pub struct ShapeInput {
    pub path: BezPath,
    /// Group id. Shapes with the same id are kept together by the planner and filled together.
    pub group: Option<u32>,
    /// Manual cut order priority (lower first). Used when the planner orders by priority.
    pub priority: i32,
    /// Power scale, 0..=1 (1 = full layer power).
    pub power_scale: f64,
    /// Forced start point: the cut starts at the path point nearest to this.
    pub start_point: Option<Point>,
    pub direction: Option<ForcedDirection>,
    /// Manually placed tabs (line mode). Each point snaps to the nearest point on the path.
    pub tabs: Vec<Point>,
}

impl ShapeInput {
    pub fn new(path: BezPath) -> Self {
        Self { path, group: None, priority: 0, power_scale: 1.0, start_point: None, direction: None, tabs: Vec::new() }
    }
}

/// One raster scanline already in millimetres, in the image's scan frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scanline {
    pub y: f64,
    /// `(x0, x1, power)` runs. `power` is 0..=1 and maps to the layer's min..max power;
    /// 0 means blank (laser off).
    pub runs: Vec<(f64, f64, f64)>,
}

/// Raster data for an image on an image-mode layer.
#[derive(Clone, Debug)]
pub struct ImageInput {
    pub scanlines: Vec<Scanline>,
    /// Maps the scan frame to design coordinates (for scan angle or placement). Identity when
    /// the scanlines are already horizontal design coordinates.
    pub transform: Affine,
    pub group: Option<u32>,
    pub priority: i32,
}

impl ImageInput {
    pub fn new(scanlines: Vec<Scanline>) -> Self {
        Self { scanlines, transform: Affine::IDENTITY, group: None, priority: 0 }
    }
}

/// Layer mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutMode {
    Line,
    Fill,
    FillLine,
    OffsetFill,
    Image,
}

/// Settings of one layer or sub-layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutSettings {
    /// Sub-layers can be switched off.
    pub enabled: bool,
    pub mode: CutMode,
    /// mm/s.
    pub speed: f64,
    /// Percent.
    pub max_power: f64,
    /// Percent. Used for ramps and grayscale.
    pub min_power: f64,
    pub passes: u32,
    pub air: bool,
    /// 1 or 2.
    pub laser: u8,
    /// GRBL constant power (M3) instead of dynamic power (M4).
    pub constant_power: bool,
    pub z: ZSettings,
    pub line: LineSettings,
    pub fill: FillSettings,
    pub image: ImageSettings,
}

impl CutSettings {
    pub fn line(speed: f64, power: f64) -> Self {
        Self { mode: CutMode::Line, speed, max_power: power, ..Self::default() }
    }

    pub fn fill(speed: f64, power: f64, interval: f64) -> Self {
        let mut s = Self { mode: CutMode::Fill, speed, max_power: power, ..Self::default() };
        s.fill.interval = interval;
        s
    }

    pub fn image(speed: f64, max_power: f64, min_power: f64) -> Self {
        Self { mode: CutMode::Image, speed, max_power, min_power, ..Self::default() }
    }
}

impl Default for CutSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: CutMode::Line,
            speed: 10.0,
            max_power: 20.0,
            min_power: 0.0,
            passes: 1,
            air: false,
            laser: 1,
            constant_power: false,
            z: ZSettings::default(),
            line: LineSettings::default(),
            fill: FillSettings::default(),
            image: ImageSettings::default(),
        }
    }
}

/// Z offset and step per pass. Only emitted when `enabled` (the device has a Z axis).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ZSettings {
    pub enabled: bool,
    /// Z for the first pass, mm.
    pub offset: f64,
    /// Added to Z after every pass (negative moves the head down into the material).
    pub step_per_pass: f64,
}

/// Lead-in and lead-out style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeadStyle {
    Line,
    Arc,
}

/// Lead-in or lead-out, generated on the waste side of closed shapes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lead {
    /// Length in mm (arc length for arcs).
    pub length: f64,
    /// Line style: angle away from the path tangent in degrees (0 = straight on, 90 = square).
    pub angle_deg: f64,
    pub style: LeadStyle,
}

/// Perforation: alternate cut and skip lengths.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Perforation {
    pub cut: f64,
    pub skip: f64,
}

/// Dot mode: fire stationary dots along the path.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DotMode {
    pub time_ms: f64,
    pub spacing: f64,
}

/// Tabs (bridges) on closed shapes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSettings {
    pub enabled: bool,
    /// Target spacing between tabs, mm.
    pub spacing: f64,
    pub min_count: u32,
    pub max_count: u32,
    /// Tab width along the path, mm.
    pub size: f64,
    /// Power for partly cut tabs (percent). `None` = laser off over tabs.
    pub tab_power: Option<f64>,
    /// No tabs on shapes inside other shapes.
    pub skip_inner: bool,
}

impl Default for TabSettings {
    fn default() -> Self {
        Self { enabled: false, spacing: 50.0, min_count: 2, max_count: 10, size: 0.5, tab_power: None, skip_inner: false }
    }
}

/// Line mode settings. Kerf offset is applied by the caller before handing shapes over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LineSettings {
    pub perforation: Option<Perforation>,
    pub tabs: TabSettings,
    pub start_pause_ms: f64,
    pub end_pause_ms: f64,
    /// Fire during the start pause at `pause_power`.
    pub cut_through_start: bool,
    /// Fire during the end pause at `pause_power`.
    pub cut_through_end: bool,
    /// Percent.
    pub pause_power: f64,
    /// Closed shapes: continue past the start point by this distance, mm.
    pub overcut: f64,
    pub lead_in: Option<Lead>,
    pub lead_out: Option<Lead>,
    pub dot_mode: Option<DotMode>,
}

/// Fill rule for scanline fills.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    #[default]
    EvenOdd,
    NonZero,
}

/// How shapes on a fill layer are grouped into one scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillGrouping {
    /// Every closed shape on the layer is scanned together.
    #[default]
    AllAtOnce,
    /// Shapes in the same group (and shapes nested inside them) are scanned together.
    GroupsTogether,
    /// Each outer shape (with the holes inside it) is scanned on its own.
    Individually,
}

/// Overscan beyond each scan line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Overscan {
    #[default]
    None,
    /// Fixed distance in mm.
    Distance(f64),
    /// Percent of the scan speed: `distance = speed_mm_s * percent / 100`.
    Percent(f64),
}

impl Overscan {
    /// Overscan distance in mm for a scan speed in mm/s.
    pub fn distance(self, speed: f64) -> f64 {
        match self {
            Overscan::None => 0.0,
            Overscan::Distance(d) => d.max(0.0),
            Overscan::Percent(p) => (speed * p / 100.0).max(0.0),
        }
    }
}

/// Fill and offset fill settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillSettings {
    /// Line interval, mm.
    pub interval: f64,
    /// Scan angle in degrees (any angle).
    pub angle_deg: f64,
    /// Added to the scan angle every pass.
    pub angle_step_per_pass: f64,
    pub bidirectional: bool,
    pub cross_hatch: bool,
    pub overscan: Overscan,
    pub fill_rule: FillRule,
    pub grouping: FillGrouping,
    /// Fill connected regions one after the other instead of whole scan lines.
    pub flood_fill: bool,
    /// Power ramp from min to max power at span ends, mm. 0 = off.
    pub ramp_length: f64,
    /// Also ramp where a span meets the outermost edge of the scan.
    pub ramp_outer_edge: bool,
    /// Gaps longer than this inside a scan line are crossed with a rapid move, mm.
    pub rapid_gap: f64,
}

impl Default for FillSettings {
    fn default() -> Self {
        Self {
            interval: 0.1,
            angle_deg: 0.0,
            angle_step_per_pass: 0.0,
            bidirectional: true,
            cross_hatch: false,
            overscan: Overscan::None,
            fill_rule: FillRule::EvenOdd,
            grouping: FillGrouping::AllAtOnce,
            flood_fill: false,
            ramp_length: 0.0,
            ramp_outer_edge: true,
            rapid_gap: 10.0,
        }
    }
}

/// Image mode settings (the scanlines come from the caller).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageSettings {
    pub bidirectional: bool,
    pub overscan: Overscan,
    /// Skip blank scanlines and blank ends of scanlines.
    pub skip_blanks: bool,
    /// Blank gaps longer than this are crossed with a rapid move when skipping blanks, mm.
    pub rapid_gap: f64,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self { bidirectional: true, overscan: Overscan::None, skip_blanks: true, rapid_gap: 10.0 }
    }
}
