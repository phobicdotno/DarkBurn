//! The galvo device settings as DarkBurn stores them in a device profile [DS-18, GALVO-02..08,
//! GALVO-11, GALVO-12, GALVO-15], and their conversion to the `darkburn-galvo` library types.
//!
//! The library types carry no serde, so this is the saved form. Safety: `to_library` never turns
//! on `enable_laser_at_init`; the laser is enabled only when a job starts after an explicit arm.

use darkburn_galvo as galvo;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Fiber,
    Mopa,
    Co2,
    Uv,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Fiber, Source::Mopa, Source::Co2, Source::Uv];
    pub fn label(self) -> &'static str {
        match self {
            Source::Fiber => "Fibre (Q-switched)",
            Source::Mopa => "MOPA fibre",
            Source::Co2 => "CO2",
            Source::Uv => "UV",
        }
    }
    pub fn to_library(self) -> galvo::LaserSource {
        match self {
            Source::Fiber => galvo::LaserSource::Fiber,
            Source::Mopa => galvo::LaserSource::Mopa,
            Source::Co2 => galvo::LaserSource::Co2,
            Source::Uv => galvo::LaserSource::Uv,
        }
    }
    /// The Q-pulse width only means something on a MOPA source.
    pub fn has_q_pulse(self) -> bool {
        self == Source::Mopa
    }
}

/// Calibration of one galvo mirror [GALVO-04].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cal {
    pub scale: f64,
    pub skew: f64,
    pub bulge: f64,
    pub trapezoid: f64,
    pub negate: bool,
    pub is_x: bool,
}

impl Cal {
    pub const fn identity(is_x: bool) -> Self {
        Cal { scale: 1.0, skew: 0.0, bulge: 0.0, trapezoid: 0.0, negate: false, is_x }
    }
    fn to_library(self) -> galvo::GalvoCal {
        galvo::GalvoCal { scale: self.scale, skew: self.skew, bulge: self.bulge, trapezoid: self.trapezoid, negate: self.negate, is_x: self.is_x }
    }
}

impl Default for Cal {
    fn default() -> Self {
        Cal::identity(true)
    }
}

/// Timing parameters [GALVO-07].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timings {
    pub jump_speed_mm_s: f64,
    pub min_jump_delay: i32,
    pub max_jump_delay: i32,
    pub jump_distance_limit_mm: f64,
    pub laser_on_tc: i32,
    pub laser_off_tc: i32,
    pub end_tc: i32,
    pub polygon_tc: i32,
}

impl Default for Timings {
    fn default() -> Self {
        let t = galvo::Timings::default();
        Timings {
            jump_speed_mm_s: t.jump_speed_mm_s,
            min_jump_delay: t.min_jump_delay,
            max_jump_delay: t.max_jump_delay,
            jump_distance_limit_mm: t.jump_distance_limit_mm,
            laser_on_tc: t.laser_on_tc,
            laser_off_tc: t.laser_off_tc,
            end_tc: t.end_tc,
            polygon_tc: t.polygon_tc,
        }
    }
}

impl Timings {
    pub fn to_library(self) -> galvo::Timings {
        galvo::Timings {
            jump_speed_mm_s: self.jump_speed_mm_s,
            min_jump_delay: self.min_jump_delay,
            max_jump_delay: self.max_jump_delay,
            jump_distance_limit_mm: self.jump_distance_limit_mm,
            laser_on_tc: self.laser_on_tc,
            laser_off_tc: self.laser_off_tc,
            end_tc: self.end_tc,
            polygon_tc: self.polygon_tc,
        }
    }
}

/// Input and output port wiring [GALVO-11, GALVO-12].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ports {
    pub light_bit: u8,
    pub mark_bit: u8,
    pub foot_bit: Option<u8>,
    pub foot_active_low: bool,
    pub door_bit: Option<u8>,
    pub door_open_is_low: bool,
    pub done_bit: Option<u8>,
    pub done_ms: u32,
}

impl Default for Ports {
    fn default() -> Self {
        let p = galvo::PortConfig::default();
        Ports {
            light_bit: p.light_bit,
            mark_bit: p.mark_bit,
            foot_bit: p.foot_bit,
            foot_active_low: p.foot_active_low,
            door_bit: p.door_bit,
            door_open_is_low: p.door_open_is_low,
            done_bit: p.done_bit,
            done_ms: p.done_ms,
        }
    }
}

impl Ports {
    pub fn to_library(self) -> galvo::PortConfig {
        galvo::PortConfig {
            light_bit: self.light_bit.min(15),
            mark_bit: self.mark_bit.min(15),
            foot_bit: self.foot_bit.map(|b| b.min(15)),
            foot_active_low: self.foot_active_low,
            door_bit: self.door_bit.map(|b| b.min(15)),
            door_open_is_low: self.door_open_is_low,
            done_bit: self.done_bit.map(|b| b.min(15)),
            done_ms: self.done_ms,
        }
    }
}

/// Repeat marking [GALVO-13, LT-17].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repeat {
    pub count: u32,
    /// Advance an external axis between repeats.
    pub axis_on: bool,
    pub axis: u8,
    pub axis_step_mm: f64,
    pub wait_foot_each: bool,
}

impl Default for Repeat {
    fn default() -> Self {
        Repeat { count: 1, axis_on: false, axis: 0, axis_step_mm: 0.0, wait_foot_each: false }
    }
}

/// Rotary runner options [GALVO-14].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rotary {
    pub on: bool,
    pub by_shape: bool,
    pub max_size_mm: f64,
    pub axis: u8,
}

impl Default for Rotary {
    fn default() -> Self {
        Rotary { on: false, by_shape: false, max_size_mm: 50.0, axis: 0 }
    }
}

/// Everything the galvo Device Settings dialog edits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GalvoDeviceSettings {
    pub source: Source,
    /// Side of the square field (lens size), mm [GALVO-05].
    pub field_size_mm: f64,
    pub offset_x_mm: f64,
    pub offset_y_mm: f64,
    pub origin_lower_left: bool,
    pub cal1: Cal,
    pub cal2: Cal,
    pub flip_x: bool,
    pub flip_y: bool,
    pub swap_xy: bool,
    pub rotate_quarters: u8,
    pub timings: Timings,
    pub ports: Ports,
    pub light_speed_mm_s: f64,
    pub light_delay_dark: i32,
    pub light_delay_light: i32,
    /// PWM words of the board bring-up [DS-18].
    pub pwm_half_period: u16,
    pub pwm_pulse_width: u16,
    pub power_invert: bool,
    pub open_mo_delay_units: u16,
    /// Frame with the red light before every job [GALVO-10].
    pub require_framing: bool,
    /// External axes (0 = rotary or conveyor) [GALVO-15].
    pub axis_steps_per_mm: [f64; 2],
    pub z_steps_per_mm: f64,
    /// Starting values for a new layer [GALVO-06].
    pub default_frequency_khz: f64,
    pub default_q_pulse_ns: f64,
    /// Wobble: points per turn (size and step are per layer) [GALVO-08].
    pub wobble_points_per_rev: u32,
    /// The lens correction file in use [GALVO-03]. Empty: none.
    pub cor_path: String,
    pub repeat: Repeat,
    pub rotary: Rotary,
    pub door_protect: bool,
    pub optimize: bool,
}

impl Default for GalvoDeviceSettings {
    fn default() -> Self {
        let l = galvo::DeviceSettings::default();
        GalvoDeviceSettings {
            source: Source::Fiber,
            field_size_mm: l.field.size_mm,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            origin_lower_left: false,
            cal1: Cal::identity(true),
            cal2: Cal::identity(false),
            flip_x: false,
            flip_y: false,
            swap_xy: false,
            rotate_quarters: 0,
            timings: Timings::default(),
            ports: Ports::default(),
            light_speed_mm_s: l.light.speed_mm_s,
            light_delay_dark: l.light.delay_dark,
            light_delay_light: l.light.delay_light,
            pwm_half_period: l.init.pwm_half_period,
            pwm_pulse_width: l.init.pwm_pulse_width,
            power_invert: false,
            open_mo_delay_units: l.open_mo_delay_units,
            require_framing: false,
            axis_steps_per_mm: l.axis_steps_per_mm,
            z_steps_per_mm: l.z_steps_per_mm,
            default_frequency_khz: 30.0,
            default_q_pulse_ns: 200.0,
            wobble_points_per_rev: 12,
            cor_path: String::new(),
            repeat: Repeat::default(),
            rotary: Rotary::default(),
            door_protect: false,
            optimize: true,
        }
    }
}

impl GalvoDeviceSettings {
    /// The library settings. `enable_laser_at_init` is always off.
    pub fn to_library(&self) -> galvo::DeviceSettings {
        let base = galvo::DeviceSettings::default();
        galvo::DeviceSettings {
            source: self.source.to_library(),
            field: galvo::FieldConfig {
                size_mm: self.field_size_mm,
                offset_mm: (self.offset_x_mm, self.offset_y_mm),
                origin: if self.origin_lower_left { galvo::FieldOrigin::LowerLeft } else { galvo::FieldOrigin::Center },
                galvo1: self.cal1.to_library(),
                galvo2: self.cal2.to_library(),
                flip_x: self.flip_x,
                flip_y: self.flip_y,
                swap_xy: self.swap_xy,
                rotate_quarters: self.rotate_quarters % 4,
            },
            timings: self.timings.to_library(),
            ports: self.ports.to_library(),
            light: galvo::LightSettings { speed_mm_s: self.light_speed_mm_s, delay_dark: self.light_delay_dark, delay_light: self.light_delay_light },
            init: galvo::settings::InitWords { pwm_half_period: self.pwm_half_period, pwm_pulse_width: self.pwm_pulse_width, ..base.init },
            power_invert: self.power_invert,
            open_mo_delay_units: self.open_mo_delay_units,
            require_framing: self.require_framing,
            // The laser is enabled only when a job starts after an explicit arm.
            enable_laser_at_init: false,
            enable_z_at_init: false,
            axis_steps_per_mm: self.axis_steps_per_mm,
            z_steps_per_mm: self.z_steps_per_mm,
        }
    }

    /// Settings read from an EZCad import, over the current ones.
    pub fn apply_library(&mut self, l: &galvo::DeviceSettings) {
        self.source = match l.source {
            galvo::LaserSource::Fiber => Source::Fiber,
            galvo::LaserSource::Mopa => Source::Mopa,
            galvo::LaserSource::Co2 => Source::Co2,
            galvo::LaserSource::Uv => Source::Uv,
        };
        self.field_size_mm = l.field.size_mm;
        self.offset_x_mm = l.field.offset_mm.0;
        self.offset_y_mm = l.field.offset_mm.1;
        let c = |g: galvo::GalvoCal| Cal { scale: g.scale, skew: g.skew, bulge: g.bulge, trapezoid: g.trapezoid, negate: g.negate, is_x: g.is_x };
        self.cal1 = c(l.field.galvo1);
        self.cal2 = c(l.field.galvo2);
        self.flip_x = l.field.flip_x;
        self.flip_y = l.field.flip_y;
        self.swap_xy = l.field.swap_xy;
        self.rotate_quarters = l.field.rotate_quarters;
        let t = l.timings;
        self.timings = Timings {
            jump_speed_mm_s: t.jump_speed_mm_s,
            min_jump_delay: t.min_jump_delay,
            max_jump_delay: t.max_jump_delay,
            jump_distance_limit_mm: t.jump_distance_limit_mm,
            laser_on_tc: t.laser_on_tc,
            laser_off_tc: t.laser_off_tc,
            end_tc: t.end_tc,
            polygon_tc: t.polygon_tc,
        };
    }

    /// Problems that make the settings unusable.
    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !(self.field_size_mm.is_finite() && (5.0..=1000.0).contains(&self.field_size_mm)) {
            v.push("The field size must be between 5 and 1000 mm.".into());
        }
        for (n, c) in [("Galvo 1", &self.cal1), ("Galvo 2", &self.cal2)] {
            if !(c.scale.is_finite() && (0.1..=10.0).contains(&c.scale.abs())) {
                v.push(format!("{n} scale must be between 0.1 and 10."));
            }
        }
        if self.cal1.is_x == self.cal2.is_x {
            v.push("One galvo must be X and the other Y.".into());
        }
        for (n, c) in [("Galvo 1", &self.cal1), ("Galvo 2", &self.cal2)] {
            if !(c.skew.is_finite() && c.bulge.is_finite() && c.trapezoid.is_finite()) {
                v.push(format!("{n} skew, bulge and trapezoid must be numbers."));
            }
        }
        if !(self.offset_x_mm.is_finite() && self.offset_y_mm.is_finite()) {
            v.push("The field offsets must be numbers.".into());
        }
        if !(self.timings.jump_speed_mm_s.is_finite() && self.timings.jump_speed_mm_s > 0.0) {
            v.push("Jump speed must be above zero.".into());
        }
        if !(self.timings.jump_distance_limit_mm.is_finite() && self.timings.jump_distance_limit_mm >= 0.0) {
            v.push("Jump distance limit must be zero or more.".into());
        }
        if !(self.light_speed_mm_s.is_finite() && self.light_speed_mm_s > 0.0) {
            v.push("Light speed must be above zero.".into());
        }
        if self.repeat.count == 0 {
            v.push("Repeat count must be at least 1.".into());
        }
        if !(self.default_frequency_khz.is_finite() && self.default_frequency_khz > 0.0) {
            v.push("Frequency must be above zero.".into());
        }
        if !(self.default_q_pulse_ns.is_finite() && self.default_q_pulse_ns >= 0.0) {
            v.push("Pulse width must be zero or more.".into());
        }
        v
    }

    /// The lens table named by `cor_path`, if any.
    pub fn load_lens(&self) -> Result<Option<galvo::CorTable>, String> {
        if self.cor_path.trim().is_empty() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.cor_path).map_err(|e| format!("Could not read {}: {e}", self.cor_path))?;
        galvo::CorTable::parse(&bytes).map(Some).map_err(|e| format!("{}: {e}", self.cor_path))
    }

    /// The wobble of a layer, when it asks for one [GALVO-08].
    pub fn wobble_for(&self, g: &darkburn_core::layer::Galvo) -> Option<galvo::Wobble> {
        g.wobble.then(|| galvo::Wobble { size_mm: g.wobble_size, step_mm: g.wobble_step, points_per_rev: self.wobble_points_per_rev.max(3) })
    }
}
