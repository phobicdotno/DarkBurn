//! Laser source, mark parameters, timings, device settings and the unit conversions between
//! them and the words on the wire.
//!
//! Conversions follow MeerK40t's `balormk` driver (MIT, see `ATTRIBUTION.md`):
//! fibre `QSwitchPeriod = 20000 / kHz` and `MarkCurrent = power * 0xFFF / 100`; CO2 and UV
//! `MarkFreq = 10000 / kHz` and `MarkPowerRatio = 200 * power / kHz`; speed in galvo units per
//! millisecond.

use crate::codec::{ListCmd, SingleCmd};
use crate::field::FieldConfig;

/// The kind of laser source on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LaserSource {
    /// Q-switched fibre (Raycus, IPG, JPT, ...).
    #[default]
    Fiber,
    /// MOPA fibre: as `Fiber` plus an adjustable pulse width.
    Mopa,
    Co2,
    Uv,
}

impl LaserSource {
    pub fn is_fiber(self) -> bool {
        matches!(self, Self::Fiber | Self::Mopa)
    }
}

/// Galvo timing parameters. Delays are in microseconds except `polygon_tc` and `end_tc`, which
/// the board takes in 10 microsecond units (DarkBurn divides by 10 on the way out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timings {
    /// Jump (laser off) speed in mm/s.
    pub jump_speed_mm_s: f64,
    /// Settle delay after a short jump.
    pub min_jump_delay: i32,
    /// Settle delay after a jump longer than `jump_distance_limit_mm`.
    pub max_jump_delay: i32,
    pub jump_distance_limit_mm: f64,
    pub laser_on_tc: i32,
    pub laser_off_tc: i32,
    /// Delay after the last mark of every path.
    pub end_tc: i32,
    /// Delay at every vertex inside a path.
    pub polygon_tc: i32,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            jump_speed_mm_s: 2000.0,
            min_jump_delay: 8,
            max_jump_delay: 200,
            jump_distance_limit_mm: 10.0,
            laser_on_tc: 100,
            laser_off_tc: 100,
            end_tc: 300,
            polygon_tc: 100,
        }
    }
}

/// Everything a layer needs to say how to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkParams {
    pub frequency_khz: f64,
    /// Q-pulse width in ns (MOPA only; ignored for other sources).
    pub q_pulse_ns: Option<u32>,
    /// 0..=100.
    pub power_pct: f64,
    pub speed_mm_s: f64,
    /// Per-layer timing override; the device defaults are used when `None`.
    pub timings: Option<Timings>,
}

impl Default for MarkParams {
    fn default() -> Self {
        Self { frequency_khz: 30.0, q_pulse_ns: None, power_pct: 50.0, speed_mm_s: 500.0, timings: None }
    }
}

/// Input and output port wiring. Bits are 0..=15 of the port word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortConfig {
    /// Bit that turns the red pointer on.
    pub light_bit: u8,
    /// Bit held high while a job runs.
    pub mark_bit: u8,
    /// Foot switch input and whether it reads low when pressed.
    pub foot_bit: Option<u8>,
    pub foot_active_low: bool,
    /// Door switch input and whether it reads low when the door is OPEN.
    pub door_bit: Option<u8>,
    pub door_open_is_low: bool,
    /// Done-marking output pulsed after a job, and for how long.
    pub done_bit: Option<u8>,
    pub done_ms: u32,
}

impl Default for PortConfig {
    fn default() -> Self {
        Self {
            light_bit: 8,
            mark_bit: 0,
            foot_bit: Some(15),
            foot_active_low: true,
            door_bit: None,
            door_open_is_low: true,
            done_bit: None,
            done_ms: 500,
        }
    }
}

impl PortConfig {
    /// A bit number the port does not have never reads as pressed.
    pub fn foot_pressed(&self, port_bits: u16) -> bool {
        match self.foot_bit {
            Some(b) if b < 16 => ((port_bits >> b) & 1 == 1) != self.foot_active_low,
            _ => false,
        }
    }

    /// A bit number the port does not have reads as an open door: a door switch that cannot be
    /// watched must not pass for a closed door.
    pub fn door_open(&self, port_bits: u16) -> bool {
        match self.door_bit {
            Some(b) if b < 16 => ((port_bits >> b) & 1 == 1) != self.door_open_is_low,
            Some(_) => true,
            None => false,
        }
    }

    /// Checks that every bit number is one the 16-bit port has.
    pub fn validate(&self) -> Result<(), String> {
        let bits = [
            ("red light", Some(self.light_bit)),
            ("marking", Some(self.mark_bit)),
            ("foot switch", self.foot_bit),
            ("door switch", self.door_bit),
            ("done signal", self.done_bit),
        ];
        for (name, bit) in bits {
            if let Some(b) = bit.filter(|b| *b > 15) {
                return Err(format!("The {name} port bit is {b}; port bits go from 0 to 15."));
            }
        }
        Ok(())
    }
}

/// Frame (red light) defaults on the device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSettings {
    pub speed_mm_s: f64,
    /// Settle delays at the corners, microseconds.
    pub delay_dark: i32,
    pub delay_light: i32,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self { speed_mm_s: 3000.0, delay_dark: 1, delay_light: 1 }
    }
}

/// Board bring-up words (defaults from MeerK40t).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitWords {
    pub control_mode: u16,
    pub laser_mode: u16,
    pub delay_mode: u16,
    pub timing_mode: u16,
    pub standby: (u16, u16),
    pub first_pulse_killer: u16,
    pub pwm_half_period: u16,
    pub pwm_pulse_width: u16,
    pub fpk2: [u16; 4],
    pub fly_res: [u16; 4],
}

impl Default for InitWords {
    fn default() -> Self {
        Self {
            control_mode: 0,
            laser_mode: 1,
            delay_mode: 1,
            timing_mode: 1,
            standby: (2000, 20),
            first_pulse_killer: 200,
            pwm_half_period: 125,
            pwm_pulse_width: 125,
            fpk2: [0xFFB, 1, 409, 100],
            fly_res: [0, 99, 1000, 25],
        }
    }
}

/// Settings of the machine profile (what the device dialog edits).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceSettings {
    pub source: LaserSource,
    pub field: FieldConfig,
    pub timings: Timings,
    pub ports: PortConfig,
    pub light: LightSettings,
    pub init: InitWords,
    /// CO2 / UV: invert power (UV boards).
    pub power_invert: bool,
    /// Fibre MO delay after opening, in 10 us units ("delay_openmo" x 100 in MeerK40t).
    pub open_mo_delay_units: u16,
    /// Frame before every job.
    pub require_framing: bool,
    /// Send `EnableLaser` while connecting (off: only when a job is armed).
    pub enable_laser_at_init: bool,
    /// Send `EnableZ` while connecting.
    pub enable_z_at_init: bool,
    /// Steps per mm of the external axes (index 0 = rotary / conveyor).
    pub axis_steps_per_mm: [f64; 2],
    /// Z steps per mm for 3D sliced passes.
    pub z_steps_per_mm: f64,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            source: LaserSource::Fiber,
            field: FieldConfig::default(),
            timings: Timings::default(),
            ports: PortConfig::default(),
            light: LightSettings::default(),
            init: InitWords::default(),
            power_invert: false,
            open_mo_delay_units: 800,
            require_framing: false,
            enable_laser_at_init: false,
            enable_z_at_init: false,
            axis_steps_per_mm: [100.0, 100.0],
            z_steps_per_mm: 100.0,
        }
    }
}

/// Galvo units per millisecond for a speed in mm/s.
pub fn speed_word(speed_mm_s: f64, units_per_mm: f64) -> u16 {
    (speed_mm_s.abs() * units_per_mm / 1000.0).round().clamp(1.0, 65535.0) as u16
}

/// Frequency word for the source.
pub fn frequency_word(source: LaserSource, khz: f64) -> u16 {
    let base = if source.is_fiber() { 20000.0 } else { 10000.0 };
    ((base / khz.max(0.001)).round() as i64 & 0xFFFF) as u16
}

/// List commands that set frequency and power for a source.
pub fn power_cmds(source: LaserSource, power_pct: f64, freq_khz: f64, invert: bool) -> Vec<ListCmd> {
    let p = power_pct.clamp(0.0, 100.0);
    match source {
        LaserSource::Fiber | LaserSource::Mopa => {
            vec![ListCmd::QSwitchPeriod(frequency_word(source, freq_khz)), ListCmd::MarkCurrent((p * 0xFFF as f64 / 100.0).round() as u16)]
        }
        LaserSource::Co2 | LaserSource::Uv => {
            let q = if source == LaserSource::Uv && invert { 100.0 - p } else { p };
            vec![
                ListCmd::MarkFreq(frequency_word(source, freq_khz)),
                ListCmd::MarkPowerRatio((200.0 * q / freq_khz.max(0.001)).round().clamp(0.0, 65535.0) as u16),
            ]
        }
    }
}

impl DeviceSettings {
    /// The immediate commands run after connecting, in order. Does not enable the laser unless
    /// `enable_laser_at_init` is set; always ends with the laser disabled and MO closed otherwise.
    pub fn init_sequence(&self) -> Vec<SingleCmd> {
        let i = &self.init;
        let mut v = vec![
            SingleCmd::GetSerialNo,
            SingleCmd::GetVersion,
            SingleCmd::Reset,
            SingleCmd::DisableLaser,
            SingleCmd::LaserSignalOff,
            SingleCmd::SetControlMode(i.control_mode),
            SingleCmd::SetLaserMode(i.laser_mode),
            SingleCmd::SetDelayMode(i.delay_mode),
            SingleCmd::SetTiming(i.timing_mode),
            SingleCmd::SetStandby(i.standby.0, i.standby.1),
            SingleCmd::SetFirstPulseKiller(i.first_pulse_killer),
            SingleCmd::SetPwmHalfPeriod(i.pwm_half_period),
            SingleCmd::SetPwmPulseWidth(i.pwm_pulse_width),
            SingleCmd::FiberSetMo(false),
            SingleCmd::SetFpkParam2(i.fpk2),
            SingleCmd::SetFlyRes(i.fly_res),
        ];
        if self.enable_z_at_init {
            v.push(SingleCmd::EnableZ);
            v.push(SingleCmd::WriteAnalogPort1(0x7FF));
        }
        if self.enable_laser_at_init {
            v.push(SingleCmd::EnableLaser);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_and_power_words() {
        assert_eq!(frequency_word(LaserSource::Fiber, 30.0), 667);
        assert_eq!(frequency_word(LaserSource::Co2, 20.0), 500);
        assert_eq!(power_cmds(LaserSource::Fiber, 50.0, 20.0, false), vec![ListCmd::QSwitchPeriod(1000), ListCmd::MarkCurrent(2048)]);
        assert_eq!(power_cmds(LaserSource::Co2, 50.0, 20.0, false), vec![ListCmd::MarkFreq(500), ListCmd::MarkPowerRatio(500)]);
        assert_eq!(power_cmds(LaserSource::Uv, 30.0, 20.0, true), vec![ListCmd::MarkFreq(500), ListCmd::MarkPowerRatio(700)]);
        // power is clamped
        assert_eq!(power_cmds(LaserSource::Fiber, 250.0, 20.0, false)[1], ListCmd::MarkCurrent(0xFFF));
    }

    #[test]
    fn speed_word_scales_with_field() {
        // 110 mm lens: 595.77 units/mm; 1000 mm/s -> 596 units/ms
        assert_eq!(speed_word(1000.0, 65535.0 / 110.0), 596);
        assert_eq!(speed_word(0.0, 600.0), 1);
        assert_eq!(speed_word(1e9, 600.0), 65535);
    }

    #[test]
    fn port_polarity() {
        let p = PortConfig { foot_bit: Some(15), foot_active_low: true, door_bit: Some(3), door_open_is_low: true, ..Default::default() };
        assert!(p.foot_pressed(0x0000));
        assert!(!p.foot_pressed(0x8000));
        assert!(p.door_open(0x0000));
        assert!(!p.door_open(0x0008));
        let none = PortConfig { foot_bit: None, door_bit: None, ..Default::default() };
        assert!(!none.foot_pressed(0) && !none.door_open(0));
    }

    #[test]
    fn init_sequence_does_not_enable_the_laser() {
        let s = DeviceSettings::default();
        assert!(!s.init_sequence().contains(&SingleCmd::EnableLaser));
        let on = DeviceSettings { enable_laser_at_init: true, ..Default::default() };
        assert_eq!(on.init_sequence().last(), Some(&SingleCmd::EnableLaser));
        assert!(s.init_sequence().contains(&SingleCmd::DisableLaser));
    }
}
