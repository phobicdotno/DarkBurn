//! The device profile: every Device Settings field for G-code machines (DS-01..DS-15, DS-19)
//! and the per-device state of DEV-07, with JSON import and export (DEV-05).
//!
//! The JSON file is `{"darkburn_device": 1, "profile": {...}}`; a bare profile object (or a
//! list of profiles under `"profiles"`) is accepted too. Unknown fields are ignored and
//! missing fields take their defaults, so older and newer files load.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::machine::{GcodeMachine, MachineError, MachineFamily};

/// Current profile file version.
pub const PROFILE_VERSION: u32 = 1;

/// The controller type chosen in the New Device wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DeviceKind {
    #[default]
    Grbl,
    GrblHal,
    FluidNc,
    Smoothie,
    Marlin,
    LinuxCnc,
    Snapmaker,
    Custom,
    /// DEV-08: the editor without a laser.
    DesignOnly,
}

impl DeviceKind {
    pub fn family(self) -> Option<MachineFamily> {
        Some(match self {
            Self::Grbl => MachineFamily::Grbl,
            Self::GrblHal => MachineFamily::GrblHal,
            Self::FluidNc => MachineFamily::FluidNc,
            Self::Smoothie => MachineFamily::Smoothie,
            Self::Marlin => MachineFamily::Marlin,
            Self::LinuxCnc => MachineFamily::LinuxCnc,
            Self::Snapmaker => MachineFamily::Snapmaker,
            Self::Custom => MachineFamily::Custom,
            Self::DesignOnly => return None,
        })
    }
    pub fn label(self) -> &'static str {
        self.family().map_or("Design only", MachineFamily::label)
    }
    /// A live driver exists for this kind (LinuxCNC is file output only).
    pub fn has_live_driver(self) -> bool {
        !matches!(self, Self::LinuxCnc | Self::DesignOnly)
    }
}

/// How the computer reaches the machine (CONN-01, CONN-02).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Connection {
    #[default]
    None,
    Serial {
        port: String,
        baud: u32,
    },
    Tcp {
        host: String,
        port: u16,
    },
}

/// Which corner of the work area is X0 Y0 (DS-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OriginCorner {
    #[default]
    FrontLeft,
    FrontRight,
    BackLeft,
    BackRight,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Units {
    #[default]
    Millimetres,
    Inches,
}

/// Red-dot pointer offset in mm (DS-03).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PointerOffset {
    pub x: f64,
    pub y: f64,
}

/// DS-04.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ZSettings {
    pub enabled: bool,
    pub reverse: bool,
    pub relative_moves_only: bool,
    pub optimize_moves: bool,
}

/// DS-06.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JobChecklist {
    pub enabled: bool,
    pub items: Vec<String>,
}

/// One row of the scanning offset table (DS-08): at `speed` mm/s the reverse scan line shifts
/// by `offset` mm (and `interval_mm`-spaced lines are not affected).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanOffset {
    pub speed_mm_s: f64,
    pub offset_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanningOffsets {
    pub enabled: bool,
    pub table: Vec<ScanOffset>,
}

impl ScanningOffsets {
    /// Offset for `speed_mm_s`, interpolated between table rows (clamped outside the table).
    pub fn offset_for(&self, speed_mm_s: f64) -> f64 {
        if !self.enabled || self.table.is_empty() {
            return 0.0;
        }
        let mut rows = self.table.clone();
        rows.sort_by(|a, b| a.speed_mm_s.total_cmp(&b.speed_mm_s));
        if speed_mm_s <= rows[0].speed_mm_s {
            return rows[0].offset_mm;
        }
        for w in rows.windows(2) {
            if speed_mm_s <= w[1].speed_mm_s {
                let span = w[1].speed_mm_s - w[0].speed_mm_s;
                if span <= 0.0 {
                    return w[1].offset_mm;
                }
                let t = (speed_mm_s - w[0].speed_mm_s) / span;
                return w[0].offset_mm + t * (w[1].offset_mm - w[0].offset_mm);
            }
        }
        rows[rows.len() - 1].offset_mm
    }
}

/// DS-13.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TransferMode {
    /// Several lines in flight (character counting on GRBL, a small line window elsewhere).
    #[default]
    Buffered,
    /// One line at a time.
    Synchronous,
}

/// How a Marlin-style controller is told the laser power (GCODE-02).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MarlinPowerMode {
    /// `M3 S<n>` constant power; `M5` off.
    #[default]
    M3,
    /// `M4 S<n>` dynamic power.
    M4,
    /// `M3 I` / `M4 I` inline mode: `S` rides on every `G1`.
    Inline,
    /// Laser on a fan output: `M106 P<tool> S<0..255>`, `M107 P<tool>` off.
    Fan,
    /// `M3 P<percent>` / `M5`: the Snapmaker 2.0 laser module takes its power in percent
    /// (GCODE-04).
    Percent,
}

/// Air assist commands (DS-12). Empty strings mean no air assist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AirAssist {
    pub on: String,
    pub off: String,
}

impl Default for AirAssist {
    fn default() -> Self {
        Self { on: "M8".into(), off: "M9".into() }
    }
}

impl AirAssist {
    pub fn is_set(&self) -> bool {
        !self.on.trim().is_empty()
    }
}

/// User-defined command templates for a custom device (GCODE-05). Placeholders: `{dx} {dy}
/// {dz} {feed}` for jog, `{x} {y}` for moves, `{s}` (S value) and `{percent}` for fire, `{power}`.
/// An empty template means "not supported".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomTemplates {
    pub home: String,
    pub jog: String,
    pub pause: String,
    pub resume: String,
    pub stop: String,
    pub fire_on: String,
    pub fire_off: String,
    /// Sent as raw bytes every poll; the reply must be a `<State,MPos:x,y,z>` report and must
    /// not be followed by `ok`. Empty: no polling.
    pub status: String,
    pub unlock: String,
    pub laser_off: String,
    /// Sent after the last line of a job; its `ok` ends the job. Empty: end on the last `ok`.
    pub finish_sync: String,
}

impl Default for CustomTemplates {
    fn default() -> Self {
        Self {
            home: "G28".into(),
            jog: "G91\nG0 X{dx} Y{dy} Z{dz} F{feed}\nG90".into(),
            pause: String::new(),
            resume: String::new(),
            stop: String::new(),
            fire_on: "M3 S{s}".into(),
            fire_off: "M5".into(),
            status: String::new(),
            unlock: String::new(),
            laser_off: "M5".into(),
            finish_sync: "M400".into(),
        }
    }
}

/// Snapmaker specifics (GCODE-04).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapmakerSettings {
    /// Machine model in the file header, e.g. `A350`.
    pub model: String,
    /// Write the `;Header Start` block.
    pub header: bool,
    /// Z of the focus point with a zero-thickness material, mm.
    pub focus_height_mm: f64,
    /// Material height, mm: Z is raised by this much before the job.
    pub material_height_mm: f64,
    /// Laser module name in the header.
    pub tool_head: String,
}

impl Default for SnapmakerSettings {
    fn default() -> Self {
        Self { model: "A350".into(), header: true, focus_height_mm: 0.0, material_height_mm: 0.0, tool_head: "levelTwoLaserToolheadForSM2".into() }
    }
}

/// G-code device settings (DS-09..DS-15).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GcodeSettings {
    /// DS-09
    pub auto_home_on_start: bool,
    pub swap_xy: bool,
    /// DS-10: blank rows of a raster are skipped at `whitespace_speed` mm/min.
    pub fast_whitespace_scan: bool,
    pub whitespace_speed: f64,
    /// DS-11
    pub use_jog_command: bool,
    pub enable_dtr: bool,
    pub fire_button: bool,
    pub laser_on_when_framing: bool,
    pub out_of_bounds_warning: bool,
    /// GCODE-01: merge raster moves into longer ones (Smoothieware).
    pub clustering: bool,
    pub return_to_finish_position: bool,
    /// DS-12
    pub air_assist: AirAssist,
    /// S value that means 100 % power. GRBL: usually 1000; Smoothieware: 1.0; Marlin fan: 255.
    pub s_max: f64,
    pub baud: u32,
    /// DS-13
    pub transfer: TransferMode,
    /// DS-14: lines sent before and after every job.
    pub start_script: String,
    pub end_script: String,
    /// DS-15
    pub g0_for_overscan: bool,
    pub s_with_every_g1: bool,
    /// Constant power (M3) instead of dynamic (M4) for GRBL.
    pub constant_power: bool,
    pub units: Units,
    pub marlin_power: MarlinPowerMode,
    /// Fan/tool index for `M106 P<n>` (0, 1 or 2).
    pub tool_index: u8,
    /// Smoothieware: do not wait the planner out before pausing.
    pub reduce_smoothie_pause_delay: bool,
    /// Highest power the fire button may use, percent.
    pub max_fire_percent: f64,
    /// Longest the fire button keeps the laser on, seconds.
    pub max_fire_seconds: f64,
    pub custom: CustomTemplates,
    pub snapmaker: SnapmakerSettings,
}

impl Default for GcodeSettings {
    fn default() -> Self {
        Self {
            auto_home_on_start: false,
            swap_xy: false,
            fast_whitespace_scan: true,
            whitespace_speed: 6000.0,
            use_jog_command: true,
            enable_dtr: true,
            fire_button: true,
            laser_on_when_framing: false,
            out_of_bounds_warning: true,
            clustering: false,
            return_to_finish_position: false,
            air_assist: AirAssist::default(),
            s_max: 1000.0,
            baud: 115_200,
            transfer: TransferMode::Buffered,
            start_script: String::new(),
            end_script: String::new(),
            g0_for_overscan: false,
            s_with_every_g1: false,
            constant_power: false,
            units: Units::Millimetres,
            marlin_power: MarlinPowerMode::M3,
            tool_index: 0,
            reduce_smoothie_pause_delay: false,
            max_fire_percent: 10.0,
            max_fire_seconds: 10.0,
            custom: CustomTemplates::default(),
            snapmaker: SnapmakerSettings::default(),
        }
    }
}

/// DS-19: values for the time estimate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimulationSettings {
    /// mm/s^2
    pub accel_x: f64,
    pub accel_y: f64,
    /// Highest speed per axis, mm/s.
    pub max_speed_x: f64,
    pub max_speed_y: f64,
    /// Rapid (G0) speed, mm/s.
    pub rapid_speed: f64,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        Self { accel_x: 1000.0, accel_y: 1000.0, max_speed_x: 500.0, max_speed_y: 500.0, rapid_speed: 300.0 }
    }
}

/// A saved copy of the controller settings (DEV-07).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsBackupEntry {
    pub name: String,
    /// Free-form timestamp text chosen by the caller.
    pub created: String,
    pub text: String,
}

/// Per-device persisted state (DEV-07).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerDeviceState {
    pub units: Units,
    pub grid_spacing_mm: f64,
    /// Job origin mode name the UI uses ("Absolute", "UserOrigin", "CurrentPosition").
    pub cut_origin: String,
    pub material_library: Option<String>,
    pub cut_library: Option<String>,
    pub default_layers: Option<String>,
    /// Opaque panel layout blob.
    pub panel_layout: Option<String>,
    pub last_port: Option<String>,
    pub settings_backups: Vec<SettingsBackupEntry>,
}

impl Default for PerDeviceState {
    fn default() -> Self {
        Self {
            units: Units::Millimetres,
            grid_spacing_mm: 10.0,
            cut_origin: "Absolute".into(),
            material_library: None,
            cut_library: None,
            default_layers: None,
            panel_layout: None,
            last_port: None,
            settings_backups: Vec::new(),
        }
    }
}

/// A device profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    pub name: String,
    pub kind: DeviceKind,
    pub connection: Connection,
    /// DS-01: work area in mm.
    pub width_mm: f64,
    pub height_mm: f64,
    pub origin: OriginCorner,
    /// DS-02: the machine origin is in the middle or far corner and coordinates go negative.
    pub negative_coordinates: bool,
    pub pointer_offset: PointerOffset,
    pub z: ZSettings,
    /// DS-05: the length of one pulse of a partly cut tab, mm. Profiles written before the
    /// field was renamed call it `tab_pulse_width_ms` (the value was always a length).
    #[serde(alias = "tab_pulse_width_ms")]
    pub tab_pulse_width_mm: f64,
    pub checklist: JobChecklist,
    pub overwrite_files: bool,
    pub scanning_offsets: ScanningOffsets,
    pub gcode: GcodeSettings,
    pub simulation: SimulationSettings,
    pub state: PerDeviceState,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            name: "New device".into(),
            kind: DeviceKind::Grbl,
            connection: Connection::None,
            width_mm: 400.0,
            height_mm: 400.0,
            origin: OriginCorner::FrontLeft,
            negative_coordinates: false,
            pointer_offset: PointerOffset::default(),
            z: ZSettings::default(),
            tab_pulse_width_mm: 0.0,
            checklist: JobChecklist::default(),
            overwrite_files: false,
            scanning_offsets: ScanningOffsets::default(),
            gcode: GcodeSettings::default(),
            simulation: SimulationSettings::default(),
            state: PerDeviceState::default(),
        }
    }
}

/// Why a profile file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    Json(String),
    /// The file was written by a newer DarkBurn.
    NewerVersion(u32),
    /// The file holds no profile.
    Empty,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(m) => write!(f, "This is not a device file: {m}"),
            Self::NewerVersion(v) => write!(f, "This device file is from a newer version of DarkBurn (file version {v})."),
            Self::Empty => write!(f, "The file has no device in it."),
        }
    }
}

impl std::error::Error for ProfileError {}

#[derive(Serialize, Deserialize)]
struct Envelope {
    darkburn_device: u32,
    #[serde(default)]
    profile: Option<serde_json::Value>,
    #[serde(default)]
    profiles: Option<Vec<serde_json::Value>>,
}

impl DeviceProfile {
    /// A sensible starting profile for a controller type (what the New Device wizard fills in).
    pub fn for_kind(kind: DeviceKind) -> Self {
        let mut p = Self { kind, name: kind.label().to_owned(), ..Self::default() };
        match kind {
            DeviceKind::Smoothie => {
                p.gcode.s_max = 1.0;
                p.gcode.air_assist = AirAssist { on: "M106".into(), off: "M107".into() };
                p.gcode.transfer = TransferMode::Synchronous;
                p.gcode.clustering = true;
                p.gcode.use_jog_command = false;
                p.gcode.start_script = "G21\nG90".into();
                p.gcode.end_script = "M5".into();
            }
            DeviceKind::Marlin => {
                p.gcode.s_max = 255.0;
                p.gcode.baud = 250_000;
                p.gcode.air_assist = AirAssist { on: "M106 P1 S255".into(), off: "M107 P1".into() };
                p.gcode.transfer = TransferMode::Synchronous;
                p.gcode.use_jog_command = false;
                p.gcode.start_script = "G21\nG90".into();
                p.gcode.end_script = "M5".into();
            }
            DeviceKind::Snapmaker => {
                p.gcode.s_max = 255.0;
                p.gcode.baud = 115_200;
                p.gcode.air_assist = AirAssist { on: String::new(), off: String::new() };
                p.gcode.marlin_power = MarlinPowerMode::Percent;
                p.gcode.transfer = TransferMode::Synchronous;
                p.gcode.use_jog_command = false;
                p.width_mm = 320.0;
                p.height_mm = 350.0;
            }
            DeviceKind::LinuxCnc => {
                p.gcode.s_max = 100.0;
                p.gcode.air_assist = AirAssist { on: "M64 P0".into(), off: "M65 P0".into() };
                p.gcode.start_script = "G21\nG90\nG64 P0.05".into();
                p.gcode.end_script = "M5\nM2".into();
            }
            DeviceKind::Custom => {
                p.gcode.s_max = 1000.0;
                p.gcode.transfer = TransferMode::Synchronous;
                p.gcode.air_assist = AirAssist { on: String::new(), off: String::new() };
                p.gcode.start_script = "G21\nG90".into();
                p.gcode.end_script = "M5".into();
            }
            DeviceKind::Grbl | DeviceKind::GrblHal | DeviceKind::FluidNc | DeviceKind::DesignOnly => {}
        }
        p
    }

    /// Problems that stop the profile from working. An empty list means it is usable.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.name.trim().is_empty() {
            out.push("The device needs a name.".to_owned());
        }
        if self.kind == DeviceKind::DesignOnly {
            return out;
        }
        for (what, v) in [("width", self.width_mm), ("height", self.height_mm)] {
            if !(v > 0.0 && v <= 5000.0) {
                out.push(format!("The work area {what} must be between 0 and 5000 mm."));
            }
        }
        // Not endless either: every power would be written as the largest number there is.
        if !(self.gcode.s_max > 0.0 && self.gcode.s_max.is_finite()) {
            out.push("S-value max must be above 0.".to_owned());
        }
        if !(1200..=2_000_000).contains(&self.gcode.baud) {
            out.push("The baud rate looks wrong.".to_owned());
        }
        if self.gcode.tool_index > 9 {
            out.push("The tool index must be 0 to 9.".to_owned());
        }
        if !(0.0..=100.0).contains(&self.gcode.max_fire_percent) {
            out.push("The fire button power limit must be 0 to 100 %.".to_owned());
        }
        if !(self.gcode.max_fire_seconds >= 0.0 && self.gcode.max_fire_seconds.is_finite()) {
            out.push("The fire button time limit must be a number of seconds, 0 or more.".to_owned());
        }
        if let Connection::Tcp { host, port } = &self.connection
            && (host.trim().is_empty() || *port == 0)
        {
            out.push("The network address needs a host and a port.".to_owned());
        }
        if self.kind == DeviceKind::Custom && self.gcode.custom.jog.trim().is_empty() {
            out.push("A custom device needs a jog command template.".to_owned());
        }
        out
    }

    /// The serial baud, from the connection when it has one, else the G-code setting.
    pub fn baud(&self) -> u32 {
        match &self.connection {
            Connection::Serial { baud, .. } => *baud,
            _ => self.gcode.baud,
        }
    }

    /// Machine travel in the coordinates the controller reports. With negative-coordinate
    /// mode (DS-02) the work area lies at negative X and Y.
    pub fn travel_limits(&self) -> darkburn_grbl::TravelLimits {
        let (w, h) = (self.width_mm, self.height_mm);
        let xy = if self.negative_coordinates { kurbo::Rect::new(-w, -h, 0.0, 0.0) } else { kurbo::Rect::new(0.0, 0.0, w, h) };
        darkburn_grbl::TravelLimits { xy, z: None }
    }

    pub fn fire_duration(&self) -> Duration {
        // "Not a number" stays what it is through `clamp`, and a duration cannot be made of it.
        let seconds = self.gcode.max_fire_seconds;
        Duration::from_secs_f64(if seconds.is_nan() { 0.0 } else { seconds.clamp(0.0, 600.0) })
    }

    /// Pretty JSON for one device (DEV-05).
    pub fn export_json(&self) -> String {
        let env = serde_json::json!({ "darkburn_device": PROFILE_VERSION, "profile": self });
        serde_json::to_string_pretty(&env).unwrap_or_default()
    }

    /// Pretty JSON for several devices.
    pub fn export_many(profiles: &[Self]) -> String {
        let env = serde_json::json!({ "darkburn_device": PROFILE_VERSION, "profiles": profiles });
        serde_json::to_string_pretty(&env).unwrap_or_default()
    }

    /// Reads one device file (or the first device of a multi-device file).
    pub fn import_json(text: &str) -> Result<Self, ProfileError> {
        Self::import_many(text)?.into_iter().next().ok_or(ProfileError::Empty)
    }

    /// Reads every device in a file.
    pub fn import_many(text: &str) -> Result<Vec<Self>, ProfileError> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| ProfileError::Json(e.to_string()))?;
        let items: Vec<serde_json::Value> = if value.get("darkburn_device").is_some() {
            let env: Envelope = serde_json::from_value(value).map_err(|e| ProfileError::Json(e.to_string()))?;
            if env.darkburn_device > PROFILE_VERSION {
                return Err(ProfileError::NewerVersion(env.darkburn_device));
            }
            env.profiles.into_iter().flatten().chain(env.profile).collect()
        } else {
            vec![value]
        };
        items
            .into_iter()
            .map(|v| {
                let mut p: Self = serde_json::from_value(v).map_err(|e| ProfileError::Json(e.to_string()))?;
                p.name = p.name.trim().to_owned();
                Ok(p)
            })
            .collect()
    }

    /// A copy named "<name> copy" (DEV-04 duplicate).
    pub fn duplicate(&self) -> Self {
        let mut p = self.clone();
        p.name = format!("{} copy", self.name);
        p.state.settings_backups.clear();
        p
    }

    /// Opens the machine described by this profile on `transport`. The transport is created
    /// by the application (serial, TCP, or an emulator in tests). LinuxCNC and design-only
    /// devices have no live link.
    pub fn connect(&self, transport: Box<dyn darkburn_grbl::Transport>) -> Result<Box<dyn GcodeMachine>, MachineError> {
        use crate::grbl_adapter::GrblAdapter;
        use crate::line::LineMachine;
        match self.kind {
            DeviceKind::Grbl | DeviceKind::GrblHal | DeviceKind::FluidNc => Ok(Box::new(GrblAdapter::connect(transport, self))),
            DeviceKind::Smoothie => Ok(Box::new(LineMachine::connect(transport, crate::smoothie::config(self)))),
            DeviceKind::Marlin => Ok(Box::new(LineMachine::connect(transport, crate::marlin::config(self)))),
            DeviceKind::Snapmaker => Ok(Box::new(LineMachine::connect(transport, crate::snapmaker::config(self)))),
            DeviceKind::Custom => Ok(Box::new(LineMachine::connect(transport, crate::custom::config(self)))),
            DeviceKind::LinuxCnc => Err(MachineError::Unsupported("LinuxCNC is file output only; use linuxcnc::write_program")),
            DeviceKind::DesignOnly => Err(MachineError::Unsupported("a design-only device has no laser")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_every_field() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
        p.name = "Garage laser".into();
        p.connection = Connection::Serial { port: "COM7".into(), baud: 250_000 };
        p.origin = OriginCorner::BackRight;
        p.negative_coordinates = true;
        p.pointer_offset = PointerOffset { x: 1.5, y: -2.0 };
        p.z = ZSettings { enabled: true, reverse: true, relative_moves_only: true, optimize_moves: true };
        p.tab_pulse_width_mm = 0.4;
        p.checklist = JobChecklist { enabled: true, items: vec!["Lid closed".into(), "Air on".into()] };
        p.overwrite_files = true;
        p.scanning_offsets = ScanningOffsets { enabled: true, table: vec![ScanOffset { speed_mm_s: 100.0, offset_mm: 0.05 }] };
        p.gcode.marlin_power = MarlinPowerMode::Inline;
        p.gcode.tool_index = 2;
        p.gcode.start_script = "G28 X Y".into();
        p.state.settings_backups.push(SettingsBackupEntry { name: "before".into(), created: "now".into(), text: "M92 X80".into() });
        let json = p.export_json();
        assert_eq!(DeviceProfile::import_json(&json).unwrap(), p);
    }

    #[test]
    fn the_tab_pulse_width_is_a_length_and_old_profiles_still_load() {
        let old = DeviceProfile::import_json(r#"{"name":"Old","kind":"Grbl","tab_pulse_width_ms":0.3}"#).unwrap();
        assert_eq!(old.tab_pulse_width_mm, 0.3);
        let json = old.export_json();
        assert!(json.contains("tab_pulse_width_mm") && !json.contains("tab_pulse_width_ms"), "{json}");
        assert_eq!(DeviceProfile::import_json(&json).unwrap().tab_pulse_width_mm, 0.3);
    }

    #[test]
    fn bare_profile_and_missing_fields_use_defaults() {
        let p = DeviceProfile::import_json(r#"{"name":"X","kind":"Smoothie","width_mm":300.0,"future_field":1}"#).unwrap();
        assert_eq!(p.kind, DeviceKind::Smoothie);
        assert_eq!(p.width_mm, 300.0);
        assert_eq!(p.height_mm, 400.0);
        assert_eq!(p.gcode.baud, 115_200);
    }

    #[test]
    fn many_and_errors() {
        let a = DeviceProfile::for_kind(DeviceKind::Custom);
        let b = DeviceProfile::for_kind(DeviceKind::Snapmaker);
        let text = DeviceProfile::export_many(&[a.clone(), b.clone()]);
        assert_eq!(DeviceProfile::import_many(&text).unwrap(), vec![a, b]);
        assert!(matches!(DeviceProfile::import_json("nope"), Err(ProfileError::Json(_))));
        assert_eq!(DeviceProfile::import_json(r#"{"darkburn_device":99}"#), Err(ProfileError::NewerVersion(99)));
        assert_eq!(DeviceProfile::import_json(r#"{"darkburn_device":1}"#), Err(ProfileError::Empty));
    }

    #[test]
    fn validation_catches_bad_values() {
        let mut p = DeviceProfile::for_kind(DeviceKind::Marlin);
        assert!(p.validate().is_empty());
        p.width_mm = 0.0;
        p.gcode.s_max = 0.0;
        p.gcode.baud = 5;
        let v = p.validate();
        assert_eq!(v.len(), 3, "{v:?}");
        let d = DeviceProfile { kind: DeviceKind::DesignOnly, width_mm: 0.0, ..Default::default() };
        assert!(d.validate().is_empty());
    }

    #[test]
    fn scanning_offset_interpolates() {
        let s = ScanningOffsets {
            enabled: true,
            table: vec![ScanOffset { speed_mm_s: 300.0, offset_mm: 0.2 }, ScanOffset { speed_mm_s: 100.0, offset_mm: 0.0 }],
        };
        assert!((s.offset_for(200.0) - 0.1).abs() < 1e-9);
        assert_eq!(s.offset_for(10.0), 0.0);
        assert_eq!(s.offset_for(900.0), 0.2);
        assert_eq!(ScanningOffsets::default().offset_for(100.0), 0.0);
    }

    #[test]
    fn duplicate_renames_and_drops_backups() {
        let mut p = DeviceProfile::default();
        p.state.settings_backups.push(SettingsBackupEntry::default());
        let d = p.duplicate();
        assert_eq!(d.name, "New device copy");
        assert!(d.state.settings_backups.is_empty());
    }

    #[test]
    fn kind_presets() {
        assert_eq!(DeviceProfile::for_kind(DeviceKind::Smoothie).gcode.s_max, 1.0);
        assert_eq!(DeviceProfile::for_kind(DeviceKind::Marlin).gcode.baud, 250_000);
        assert!(!DeviceKind::LinuxCnc.has_live_driver());
        assert_eq!(DeviceKind::DesignOnly.family(), None);
    }
}
