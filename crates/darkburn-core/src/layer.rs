//! Layers and cut settings: 30 colour layers and 2 tool layers, each with one or more
//! sub-layers of cut settings. Every setting the Cut Settings Editor shows is stored here, even
//! those the output engine only uses in later phases.

use serde::{Deserialize, Serialize};

/// Colour layers C00 to C29.
pub const COLOUR_LAYERS: usize = 30;
/// Tool layer T1 (index 30) and T2 (index 31): never sent to the laser.
pub const T1: usize = 30;
pub const T2: usize = 31;
/// All layers.
pub const LAYER_COUNT: usize = 32;
/// The most cut settings (sub-layers) one layer can hold.
pub const MAX_SUB_LAYERS: usize = 11;

/// The colour of layer `i` as RGB. DarkBurn's own palette: well separated hues, then greys.
pub fn layer_colour(i: usize) -> [u8; 3] {
    match i {
        0 => [20, 20, 20],
        27 => [110, 110, 110],
        28 => [170, 170, 170],
        29 => [230, 230, 230],
        T1 => [255, 120, 210],
        T2 => [120, 220, 255],
        _ => {
            let h = ((i.saturating_sub(1)) as f32 * 137.508) % 360.0;
            let l = if i.is_multiple_of(2) { 0.45 } else { 0.6 };
            hsl(h, 0.75, l)
        }
    }
}

fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b)]
}

/// The short label of layer `i`: "00" to "29", "T1", "T2".
pub fn layer_label(i: usize) -> String {
    match i {
        T1 => "T1".into(),
        T2 => "T2".into(),
        _ => format!("{i:02}"),
    }
}

pub fn is_tool_layer(i: usize) -> bool {
    i == T1 || i == T2
}

/// The colour layer whose colour is closest to `rgb` (import colour mapping).
pub fn nearest_layer(rgb: [u8; 3]) -> usize {
    let d = |a: [u8; 3]| -> i32 { a.iter().zip(rgb).map(|(x, y)| (*x as i32 - y as i32).pow(2)).sum() };
    (0..COLOUR_LAYERS).min_by_key(|i| d(layer_colour(*i))).unwrap_or(0)
}

/// How a layer's shapes are burned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Line,
    Fill,
    FillLine,
    OffsetFill,
    Image,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Line, Mode::Fill, Mode::FillLine, Mode::OffsetFill, Mode::Image];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Line => "Line",
            Mode::Fill => "Fill",
            Mode::FillLine => "Fill+Line",
            Mode::OffsetFill => "Offset Fill",
            Mode::Image => "Image",
        }
    }

    /// Whether shapes on the layer are filled (drawn filled, selectable by clicking inside).
    pub fn fills(self) -> bool {
        matches!(self, Mode::Fill | Mode::FillLine | Mode::OffsetFill)
    }
}

/// Which tube a sub-layer uses on dual-laser machines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaserSelect {
    #[default]
    Laser1,
    Laser2,
    Both,
}

/// When a DSP controller switches air assist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AirMode {
    #[default]
    ByLayer,
    WithLaser,
}

/// Which source a Gweike M-series layer uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GweikeSource {
    #[default]
    Fibre,
    Co2,
}

/// Assist gas of a Gweike fibre layer (the codes of the Mlaser files, 0..=5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GasKind {
    #[default]
    Air,
    O2,
    N2,
    HighAir,
    HighO2,
    HighN2,
}

impl GasKind {
    pub const ALL: [GasKind; 6] = [GasKind::Air, GasKind::O2, GasKind::N2, GasKind::HighAir, GasKind::HighO2, GasKind::HighN2];
    pub fn label(self) -> &'static str {
        match self {
            GasKind::Air => "Air",
            GasKind::O2 => "Oxygen",
            GasKind::N2 => "Nitrogen",
            GasKind::HighAir => "Air, high pressure",
            GasKind::HighO2 => "Oxygen, high pressure",
            GasKind::HighN2 => "Nitrogen, high pressure",
        }
    }
}

/// One pierce stage of a Gweike fibre layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PierceStep {
    /// Nozzle height, mm.
    pub height_mm: f64,
    /// Duty, percent.
    pub duty_pct: f64,
    pub freq_hz: f64,
    pub peak_current_pct: f64,
    pub gas: GasKind,
    pub gas_pressure_bar: f64,
    /// Laser-on time, ms.
    pub time_ms: f64,
}

impl Default for PierceStep {
    fn default() -> Self {
        PierceStep { height_mm: 5.0, duty_pct: 50.0, freq_hz: 500.0, peak_current_pct: 100.0, gas: GasKind::Air, gas_pressure_bar: 5.0, time_ms: 1000.0 }
    }
}

/// Gweike M-series layer values (fibre cutting and the source pick). Stored with the layer and
/// written to the Gweike files only; DarkBurn never drives the follower, piercing or gas live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GweikeLayer {
    pub source: GweikeSource,
    // Height follower
    /// Follow the sheet with the capacitive follower.
    pub follow: bool,
    /// Nozzle height while cutting, mm.
    pub cut_height_mm: f64,
    /// Head position for fixed-height cutting (Cut Position), mm.
    pub fixed_height_mm: f64,
    /// Frog-jump lift between contours.
    pub frog_jump: bool,
    /// Lift height between contours, mm.
    pub lift_height_mm: f64,
    /// No lift between contours that start close together.
    pub short_distance_no_lift: bool,
    // Gas
    pub gas: GasKind,
    pub gas_pressure_bar: f64,
    /// Dwell after the laser is on, before motion, ms.
    pub laser_on_delay_ms: f64,
    /// Dwell at the contour end before laser off, ms.
    pub laser_off_before_delay_ms: f64,
    /// Gas blow after laser off, ms.
    pub laser_off_after_delay_ms: f64,
    pub gas_on_short_moves: bool,
    pub gas_on_whole_job: bool,
    // Source
    pub freq_hz: f64,
    pub peak_current_pct: f64,
    pub duty_pct: f64,
    // Piercing
    /// Pierce every start point of the layer first.
    pub pre_drill: bool,
    pub pierce_again_before_cut: bool,
    pub pierce: Vec<PierceStep>,
    pub slow_start: bool,
    pub slow_start_length_mm: f64,
    pub slow_start_speed_mm_s: f64,
    // Contours
    pub micro_joints: bool,
    pub micro_joint_length_mm: f64,
    pub micro_joint_spacing_mm: f64,
    /// Film-removal pass before the cut.
    pub film_removal: bool,
    /// Nozzle code such as `1.5S`, kept as text.
    pub nozzle: String,
}

impl Default for GweikeLayer {
    fn default() -> Self {
        GweikeLayer {
            source: GweikeSource::Fibre,
            follow: true,
            cut_height_mm: 1.0,
            fixed_height_mm: 20.0,
            frog_jump: true,
            lift_height_mm: 20.0,
            short_distance_no_lift: true,
            gas: GasKind::Air,
            gas_pressure_bar: 5.0,
            laser_on_delay_ms: 0.0,
            laser_off_before_delay_ms: 0.0,
            laser_off_after_delay_ms: 0.0,
            gas_on_short_moves: false,
            gas_on_whole_job: false,
            freq_hz: 5000.0,
            peak_current_pct: 100.0,
            duty_pct: 100.0,
            pre_drill: false,
            pierce_again_before_cut: false,
            pierce: Vec::new(),
            slow_start: false,
            slow_start_length_mm: 2.0,
            slow_start_speed_mm_s: 10.0,
            micro_joints: false,
            micro_joint_length_mm: 0.3,
            micro_joint_spacing_mm: 50.0,
            film_removal: false,
            nozzle: String::new(),
        }
    }
}

/// How fills group shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillGrouping {
    #[default]
    AllAtOnce,
    GroupsTogether,
    Individually,
}

/// Lead-in and lead-out style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeadStyle {
    #[default]
    Line,
    Arc,
}

/// How a bitmap turns into dots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageMode {
    Threshold,
    Ordered,
    Atkinson,
    #[default]
    Dither,
    Stucki,
    Jarvis,
    Newsprint,
    Halftone,
    Sketch,
    Grayscale,
}

impl ImageMode {
    pub const ALL: [ImageMode; 10] = [
        ImageMode::Threshold,
        ImageMode::Ordered,
        ImageMode::Atkinson,
        ImageMode::Dither,
        ImageMode::Stucki,
        ImageMode::Jarvis,
        ImageMode::Newsprint,
        ImageMode::Halftone,
        ImageMode::Sketch,
        ImageMode::Grayscale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ImageMode::Threshold => "Threshold",
            ImageMode::Ordered => "Ordered",
            ImageMode::Atkinson => "Atkinson",
            ImageMode::Dither => "Dither",
            ImageMode::Stucki => "Stucki",
            ImageMode::Jarvis => "Jarvis",
            ImageMode::Newsprint => "Newsprint",
            ImageMode::Halftone => "Halftone",
            ImageMode::Sketch => "Sketch",
            ImageMode::Grayscale => "Grayscale",
        }
    }
}

/// Lead-in or lead-out settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lead {
    pub enabled: bool,
    /// Millimetres.
    pub length: f64,
    /// Degrees from the cut direction.
    pub angle: f64,
    pub style: LeadStyle,
}

impl Default for Lead {
    fn default() -> Self {
        Lead { enabled: false, length: 1.0, angle: 90.0, style: LeadStyle::Line }
    }
}

/// Tabs (bridges) that hold parts in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tabs {
    pub enabled: bool,
    /// Space tabs evenly; otherwise only manually placed tabs are used.
    pub auto_spacing: bool,
    /// Distance between tabs in millimetres when spaced evenly.
    pub spacing: f64,
    pub min_count: u32,
    pub max_count: u32,
    /// Tab width in millimetres.
    pub size: f64,
    /// Cut through the tabs at reduced power instead of skipping them.
    pub partial: bool,
    /// Power in percent used over partly cut tabs.
    pub tab_power: f64,
    pub skip_inner: bool,
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs {
            enabled: false,
            auto_spacing: true,
            spacing: 50.0,
            min_count: 1,
            max_count: 4,
            size: 0.5,
            partial: false,
            tab_power: 20.0,
            skip_inner: true,
        }
    }
}

/// Galvo-only settings (hidden on gantry machines).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Galvo {
    pub frequency_khz: f64,
    pub q_pulse_ns: f64,
    pub pause_before_ms: f64,
    pub pause_after_ms: f64,
    pub global_passes: u32,
    pub auto_rotate: bool,
    pub wobble: bool,
    pub wobble_size: f64,
    pub wobble_step: f64,
    pub timing_override: bool,
    pub jump_speed: f64,
    pub min_jump_delay_us: f64,
    pub max_jump_delay_us: f64,
    pub jump_distance_limit: f64,
    pub laser_on_tc_us: f64,
    pub laser_off_tc_us: f64,
    pub end_tc_us: f64,
    pub polygon_tc_us: f64,
    /// Drill mode (line layers) [GALVO-19]: fire in place at points along the outline
    /// instead of tracing it.
    pub drill: bool,
    /// How long the laser fires at each drill point, ms.
    pub drill_dwell_ms: f64,
    /// Distance between drill points along the outline, mm; 0 drills only the outline's own points.
    pub drill_spacing_mm: f64,
}

impl Default for Galvo {
    fn default() -> Self {
        Galvo {
            frequency_khz: 30.0,
            q_pulse_ns: 200.0,
            pause_before_ms: 0.0,
            pause_after_ms: 0.0,
            global_passes: 1,
            auto_rotate: false,
            wobble: false,
            wobble_size: 0.5,
            wobble_step: 0.1,
            timing_override: false,
            jump_speed: 4000.0,
            min_jump_delay_us: 10.0,
            max_jump_delay_us: 400.0,
            jump_distance_limit: 10.0,
            laser_on_tc_us: 100.0,
            laser_off_tc_us: 100.0,
            end_tc_us: 300.0,
            polygon_tc_us: 100.0,
            drill: false,
            drill_dwell_ms: 1.0,
            drill_spacing_mm: 0.5,
        }
    }
}

/// One set of cut settings: a layer's main settings or one of its sub-layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutSetting {
    /// Cut setting name (shown in the editor and as a sub-layer tab).
    pub name: String,
    /// Sub-layers can be switched off without deleting them.
    pub enabled: bool,
    pub mode: Mode,
    // Shared
    /// Millimetres per second.
    pub speed: f64,
    /// Percent.
    pub max_power: f64,
    pub min_power: f64,
    /// Laser 2 power on dual-tube machines.
    pub max_power_2: f64,
    pub min_power_2: f64,
    pub laser: LaserSelect,
    pub passes: u32,
    pub air_mode: AirMode,
    /// GRBL M3 constant power instead of M4 dynamic power.
    pub constant_power: bool,
    pub z_offset: f64,
    pub z_step: f64,
    // Line
    /// Kerf offset in millimetres for closed shapes (positive = outwards).
    pub kerf: f64,
    pub perforation: bool,
    pub perf_cut: f64,
    pub perf_skip: f64,
    pub tabs: Tabs,
    pub start_pause_ms: f64,
    pub end_pause_ms: f64,
    pub cut_through_start: bool,
    pub cut_through_end: bool,
    pub cut_through_power: f64,
    pub overcut: f64,
    pub pwm_override: bool,
    pub pwm_khz: f64,
    pub ppi_enabled: bool,
    pub ppi: f64,
    pub lead_in: Lead,
    pub lead_out: Lead,
    pub dot_mode: bool,
    pub dot_time_ms: f64,
    pub dot_spacing: f64,
    pub u_offset: f64,
    // Fill and image
    pub bidirectional: bool,
    pub cross_hatch: bool,
    /// Overscan, in percent of speed or millimetres (see `overscan_percent`).
    pub overscan: f64,
    pub overscan_percent: bool,
    /// Line interval in millimetres (25.4 / lines per inch).
    pub interval: f64,
    /// Degrees.
    pub scan_angle: f64,
    pub angle_increment: f64,
    pub fill_grouping: FillGrouping,
    pub ramp_length: f64,
    pub ramp_outer: bool,
    pub flood_fill: bool,
    // Image
    pub image_mode: ImageMode,
    pub negative: bool,
    pub pass_through: bool,
    pub dot_width_correction: f64,
    pub halftone_cells: f64,
    pub halftone_angle: f64,
    pub threshold_ramp: f64,
    /// Images on this layer are merged into one scan.
    pub merge_images: bool,
    // Galvo
    pub galvo: Galvo,
    // Gweike M-series
    /// Gweike values; `None` until the user sets one. Kept when the device changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gweike: Option<GweikeLayer>,
}

impl Default for CutSetting {
    fn default() -> Self {
        CutSetting {
            name: String::new(),
            enabled: true,
            mode: Mode::Line,
            speed: 100.0,
            max_power: 20.0,
            min_power: 10.0,
            max_power_2: 20.0,
            min_power_2: 10.0,
            laser: LaserSelect::Laser1,
            passes: 1,
            air_mode: AirMode::ByLayer,
            constant_power: false,
            z_offset: 0.0,
            z_step: 0.0,
            kerf: 0.0,
            perforation: false,
            perf_cut: 1.0,
            perf_skip: 1.0,
            tabs: Tabs::default(),
            start_pause_ms: 0.0,
            end_pause_ms: 0.0,
            cut_through_start: false,
            cut_through_end: false,
            cut_through_power: 0.0,
            overcut: 0.0,
            pwm_override: false,
            pwm_khz: 20.0,
            ppi_enabled: false,
            ppi: 400.0,
            lead_in: Lead::default(),
            lead_out: Lead::default(),
            dot_mode: false,
            dot_time_ms: 1.0,
            dot_spacing: 0.1,
            u_offset: 0.0,
            bidirectional: true,
            cross_hatch: false,
            overscan: 2.5,
            overscan_percent: true,
            interval: 0.1,
            scan_angle: 0.0,
            angle_increment: 0.0,
            fill_grouping: FillGrouping::AllAtOnce,
            ramp_length: 0.0,
            ramp_outer: false,
            flood_fill: false,
            image_mode: ImageMode::Dither,
            negative: false,
            pass_through: false,
            dot_width_correction: 0.0,
            halftone_cells: 50.0,
            halftone_angle: 45.0,
            threshold_ramp: 0.0,
            merge_images: true,
            galvo: Galvo::default(),
            gweike: None,
        }
    }
}

impl CutSetting {
    /// Lines (or dots) per inch for the line interval.
    pub fn dpi(&self) -> f64 {
        if self.interval > 0.0 { 25.4 / self.interval } else { 0.0 }
    }

    /// Set the interval from lines per inch.
    pub fn set_dpi(&mut self, dpi: f64) {
        if dpi.is_finite() && dpi > 0.0 {
            self.interval = 25.4 / dpi;
        }
    }

    /// Whether the line interval is so fine it is likely a mistake (over 1000 DPI).
    pub fn very_high_dpi(&self) -> bool {
        self.dpi() > 1000.0
    }

    /// Overscan distance in millimetres at this speed (percent mode: a share of speed, as a
    /// distance the head needs to reach speed).
    pub fn overscan_mm(&self) -> f64 {
        if self.overscan_percent { self.speed * self.overscan / 100.0 } else { self.overscan }
    }

    /// Keep numbers in sensible ranges (after editing or loading).
    pub fn sanitise(&mut self) {
        let clamp = |v: &mut f64, lo: f64, hi: f64, d: f64| {
            *v = if v.is_finite() { v.clamp(lo, hi) } else { d };
        };
        clamp(&mut self.speed, 0.01, 100_000.0, 100.0);
        for p in [&mut self.max_power, &mut self.min_power, &mut self.max_power_2, &mut self.min_power_2, &mut self.cut_through_power] {
            clamp(p, 0.0, 100.0, 0.0);
        }
        clamp(&mut self.interval, 0.001, 100.0, 0.1);
        clamp(&mut self.z_offset, -100.0, 100.0, 0.0);
        self.scan_angle = if self.scan_angle.is_finite() { self.scan_angle.rem_euclid(360.0) } else { 0.0 };
        self.passes = self.passes.clamp(1, 10_000);
        // Everything else: the ranges the Cut Settings editor holds its fields to, so a value
        // from a file or a script is never one the editor could not have made (four thousand
        // million tabs, a pause of years).
        let d = CutSetting::default();
        clamp(&mut self.z_step, -100.0, 100.0, d.z_step);
        clamp(&mut self.kerf, -10.0, 10.0, d.kerf);
        clamp(&mut self.perf_cut, 0.01, 1000.0, d.perf_cut);
        clamp(&mut self.perf_skip, 0.01, 1000.0, d.perf_skip);
        clamp(&mut self.tabs.spacing, 0.1, 10_000.0, d.tabs.spacing);
        clamp(&mut self.tabs.size, 0.0, 100.0, d.tabs.size);
        clamp(&mut self.tabs.tab_power, 0.0, 100.0, d.tabs.tab_power);
        self.tabs.min_count = self.tabs.min_count.min(1000);
        self.tabs.max_count = self.tabs.max_count.min(1000);
        clamp(&mut self.start_pause_ms, 0.0, 60_000.0, d.start_pause_ms);
        clamp(&mut self.end_pause_ms, 0.0, 60_000.0, d.end_pause_ms);
        clamp(&mut self.overcut, 0.0, 100.0, d.overcut);
        clamp(&mut self.pwm_khz, 0.1, 1000.0, d.pwm_khz);
        clamp(&mut self.ppi, 1.0, 10_000.0, d.ppi);
        for l in [&mut self.lead_in, &mut self.lead_out] {
            clamp(&mut l.length, 0.0, 100.0, 1.0);
            clamp(&mut l.angle, 0.0, 180.0, 90.0);
        }
        clamp(&mut self.dot_time_ms, 0.001, 10_000.0, d.dot_time_ms);
        clamp(&mut self.dot_spacing, 0.001, 1000.0, d.dot_spacing);
        clamp(&mut self.u_offset, -1000.0, 1000.0, d.u_offset);
        clamp(&mut self.overscan, 0.0, 1000.0, d.overscan);
        clamp(&mut self.angle_increment, -360.0, 360.0, d.angle_increment);
        clamp(&mut self.ramp_length, 0.0, 100.0, d.ramp_length);
        clamp(&mut self.dot_width_correction, 0.0, 10.0, d.dot_width_correction);
        clamp(&mut self.halftone_cells, 1.0, 1000.0, d.halftone_cells);
        clamp(&mut self.halftone_angle, -360.0, 360.0, d.halftone_angle);
        clamp(&mut self.threshold_ramp, 0.0, 100.0, d.threshold_ramp);
        let (g, dg) = (&mut self.galvo, d.galvo);
        clamp(&mut g.frequency_khz, 1.0, 4000.0, dg.frequency_khz);
        clamp(&mut g.q_pulse_ns, 1.0, 10_000.0, dg.q_pulse_ns);
        clamp(&mut g.pause_before_ms, 0.0, 60_000.0, dg.pause_before_ms);
        clamp(&mut g.pause_after_ms, 0.0, 60_000.0, dg.pause_after_ms);
        g.global_passes = g.global_passes.clamp(1, 10_000);
        clamp(&mut g.wobble_size, 0.0, 10.0, dg.wobble_size);
        clamp(&mut g.wobble_step, 0.0, 10.0, dg.wobble_step);
        clamp(&mut g.jump_speed, 1.0, 100_000.0, dg.jump_speed);
        clamp(&mut g.min_jump_delay_us, 0.0, 100_000.0, dg.min_jump_delay_us);
        clamp(&mut g.max_jump_delay_us, 0.0, 100_000.0, dg.max_jump_delay_us);
        clamp(&mut g.jump_distance_limit, 0.0, 1000.0, dg.jump_distance_limit);
        clamp(&mut g.laser_on_tc_us, -10_000.0, 10_000.0, dg.laser_on_tc_us);
        clamp(&mut g.laser_off_tc_us, -10_000.0, 10_000.0, dg.laser_off_tc_us);
        clamp(&mut g.end_tc_us, 0.0, 100_000.0, dg.end_tc_us);
        clamp(&mut g.polygon_tc_us, 0.0, 100_000.0, dg.polygon_tc_us);
        clamp(&mut g.drill_dwell_ms, 0.01, 60_000.0, dg.drill_dwell_ms);
        clamp(&mut g.drill_spacing_mm, 0.0, 1000.0, dg.drill_spacing_mm);
    }
}

/// One layer in the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    /// 0..32; fixed, it is the layer's colour.
    pub index: usize,
    pub name: String,
    /// Sent to the laser.
    pub output: bool,
    /// Drawn on the canvas (and selectable).
    pub show: bool,
    pub air: bool,
    /// Tool layers: framed with the job.
    pub frame: bool,
    /// Main settings first, then sub-layers. Never empty; at most [`MAX_SUB_LAYERS`].
    pub settings: Vec<CutSetting>,
    /// Name of the material library entry this layer is linked to.
    pub library_link: Option<String>,
}

impl Default for Layer {
    fn default() -> Self {
        Layer::new(0)
    }
}

impl Layer {
    pub fn new(index: usize) -> Layer {
        let tool = is_tool_layer(index);
        Layer {
            index,
            name: if tool { layer_label(index) } else { format!("C{index:02}") },
            output: !tool,
            show: true,
            air: !tool,
            frame: false,
            settings: vec![CutSetting::default()],
            library_link: None,
        }
    }

    pub fn is_tool(&self) -> bool {
        is_tool_layer(self.index)
    }

    /// The main cut settings.
    pub fn main(&self) -> &CutSetting {
        &self.settings[0]
    }

    pub fn main_mut(&mut self) -> &mut CutSetting {
        &mut self.settings[0]
    }

    /// Whether the layer uses sub-layers (shown as "Multi").
    pub fn is_multi(&self) -> bool {
        self.settings.len() > 1
    }

    /// The mode label for the layer list.
    pub fn mode_label(&self) -> &'static str {
        if self.is_tool() {
            "Tool"
        } else if self.is_multi() {
            "Multi"
        } else {
            self.main().mode.label()
        }
    }

    /// Whether shapes on this layer are drawn filled and selectable from inside.
    pub fn fills(&self) -> bool {
        !self.is_tool() && self.settings.iter().any(|s| s.enabled && s.mode.fills())
    }

    /// Whether this layer can actually burn (tool layers never do).
    pub fn outputs(&self) -> bool {
        self.output && !self.is_tool()
    }

    /// Add a sub-layer (a copy of the last one). False when the layer is full.
    pub fn add_sub_layer(&mut self) -> bool {
        self.duplicate_sub_layer(self.settings.len() - 1)
    }

    pub fn duplicate_sub_layer(&mut self, i: usize) -> bool {
        if self.settings.len() >= MAX_SUB_LAYERS || i >= self.settings.len() {
            return false;
        }
        let copy = self.settings[i].clone();
        self.settings.insert(i + 1, copy);
        true
    }

    /// Remove sub-layer `i` (the last one cannot be removed).
    pub fn remove_sub_layer(&mut self, i: usize) -> bool {
        if self.settings.len() <= 1 || i >= self.settings.len() {
            return false;
        }
        self.settings.remove(i);
        true
    }

    /// Move sub-layer `i` by `delta` places.
    pub fn move_sub_layer(&mut self, i: usize, delta: isize) -> bool {
        let j = i as isize + delta;
        if i >= self.settings.len() || j < 0 || j as usize >= self.settings.len() {
            return false;
        }
        let s = self.settings.remove(i);
        self.settings.insert(j as usize, s);
        true
    }

    /// "Speed / power" summary for the layer list.
    pub fn speed_power(&self) -> String {
        let m = self.main();
        format!("{:.0} / {:.0}", m.speed, m.max_power)
    }
}

/// All 32 layers by index.
pub fn default_layers() -> Vec<Layer> {
    (0..LAYER_COUNT).map(Layer::new).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gweike_values_round_trip_through_a_project_file() {
        let mut d = crate::Document::new();
        let g = GweikeLayer {
            source: GweikeSource::Co2,
            cut_height_mm: 0.8,
            gas: GasKind::HighN2,
            gas_pressure_bar: 14.0,
            pre_drill: true,
            pierce: vec![PierceStep { height_mm: 6.0, time_ms: 300.0, ..Default::default() }],
            micro_joints: true,
            film_removal: true,
            nozzle: "1.5S".into(),
            ..Default::default()
        };
        d.layer_mut(0).settings[0].gweike = Some(g.clone());
        let bytes = crate::project::save(&d, "test", None).unwrap();
        let back = crate::project::load(&bytes).unwrap().doc;
        assert_eq!(back.layer(0).settings[0].gweike.as_ref(), Some(&g), "hidden Gweike values are kept");
        assert_eq!(back.layer(1).settings[0].gweike, None);
    }

    #[test]
    fn old_cut_settings_without_gweike_values_load() {
        let old = serde_json::json!({ "name": "Cut", "speed": 12.0, "max_power": 80.0 });
        let c: CutSetting = serde_json::from_value(old).unwrap();
        assert_eq!(c.gweike, None);
        assert_eq!(c.speed, 12.0);
        // A layer without Gweike values writes no `gweike` key, so older DarkBurns read it as before.
        let v = serde_json::to_value(CutSetting::default()).unwrap();
        assert!(v.get("gweike").is_none());
        // Partial Gweike values fill the rest from the defaults.
        let part: CutSetting = serde_json::from_value(serde_json::json!({ "gweike": { "gas": "o2" } })).unwrap();
        let g = part.gweike.unwrap();
        assert_eq!(g.gas, GasKind::O2);
        assert_eq!(g.source, GweikeSource::Fibre);
        assert_eq!(g.cut_height_mm, GweikeLayer::default().cut_height_mm);
    }

    #[test]
    fn thirty_colour_layers_and_two_tool_layers() {
        let layers = default_layers();
        assert_eq!(layers.len(), 32);
        assert_eq!(layers.iter().filter(|l| l.is_tool()).count(), 2);
        assert!(!layers[T1].outputs() && !layers[T2].outputs());
        assert!(layers[0].outputs());
        assert_eq!(layer_label(5), "05");
        assert_eq!(layer_label(T2), "T2");
        let colours: Vec<_> = (0..LAYER_COUNT).map(layer_colour).collect();
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b, "layer colours are distinct");
            }
        }
    }

    #[test]
    fn nearest_colour_finds_its_own_layer() {
        for i in 0..COLOUR_LAYERS {
            assert_eq!(nearest_layer(layer_colour(i)), i);
        }
        assert_eq!(nearest_layer([0, 0, 0]), 0, "black goes to layer 00");
    }

    #[test]
    fn sub_layers_add_duplicate_remove_reorder() {
        let mut l = Layer::new(1);
        assert!(!l.is_multi());
        assert_eq!(l.mode_label(), "Line");
        l.settings[0].speed = 42.0;
        assert!(l.add_sub_layer());
        assert!(l.is_multi());
        assert_eq!(l.mode_label(), "Multi");
        assert_eq!(l.settings[1].speed, 42.0);
        l.settings[1].speed = 7.0;
        assert!(l.move_sub_layer(1, -1));
        assert_eq!(l.settings[0].speed, 7.0);
        while l.add_sub_layer() {}
        assert_eq!(l.settings.len(), MAX_SUB_LAYERS);
        while l.remove_sub_layer(0) {}
        assert_eq!(l.settings.len(), 1, "the last settings stay");
        assert!(!l.move_sub_layer(0, 1));
    }

    #[test]
    fn interval_and_dpi_are_linked() {
        let mut s = CutSetting::default();
        s.set_dpi(254.0);
        assert!((s.interval - 0.1).abs() < 1e-12);
        assert!((s.dpi() - 254.0).abs() < 1e-9);
        s.interval = 0.01;
        assert!(s.very_high_dpi());
        s.speed = f64::NAN;
        s.max_power = 150.0;
        s.scan_angle = -90.0;
        s.sanitise();
        assert_eq!((s.speed, s.max_power, s.scan_angle), (100.0, 100.0, 270.0));
    }

    #[test]
    fn fill_modes_fill() {
        let mut l = Layer::new(2);
        assert!(!l.fills());
        l.main_mut().mode = Mode::Fill;
        assert!(l.fills());
        assert!(!Layer::new(T1).fills());
        let s = CutSetting { overscan: 2.0, overscan_percent: false, ..Default::default() };
        assert_eq!(s.overscan_mm(), 2.0);
    }

    #[test]
    fn old_files_missing_fields_still_load() {
        let s: CutSetting = serde_json::from_str(r#"{"speed": 12.5, "mode": "Fill"}"#).unwrap();
        assert_eq!(s.speed, 12.5);
        assert_eq!(s.mode, Mode::Fill);
        assert_eq!(s.passes, 1);
    }
}
