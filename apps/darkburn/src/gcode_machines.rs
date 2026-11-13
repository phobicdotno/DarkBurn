//! The G-code controllers of `darkburn-gcodebots` behind [`MachineHandle`] [GCODE-01..05,
//! DS-09..DS-15, DEV-02].
//!
//! * [`GcodeAdapter`] is the one [`DriverAdapter`] over the library's `GcodeMachine` trait. It
//!   serves Smoothieware, Marlin, Snapmaker (serial), the custom G-code device and Emblaser 2
//!   (Smoothie family), and, through the library's `GrblAdapter`, GRBL as well (used by the GRBL
//!   library simulator; the app's own `GrblAdapter` still serves ordinary GRBL devices because
//!   it has the `$N` settings backup). LinuxCNC has no live link: file output only.
//! * [`to_profile`] and [`from_profile`] convert between the app's [`Device`] and the library's
//!   `DeviceProfile`. There is one profile format on disk (`.dbdev`); a library JSON device file
//!   is converted on import and can be exported ([`export_library_profile`]).
//! * Jobs: the job builder writes GRBL-style G-code with neutral settings and
//!   `darkburn_gcodebots::translate` rewrites it into the device's dialect with the device's
//!   start and end scripts, homing, clustering, air commands and power style ([`job_lines`],
//!   [`job_text`]). Nothing here opens a port: real links come from `machine_api::open_real`
//!   after the user's Connect click.

#![allow(dead_code, reason = "the MCP backend and some helpers use parts of this file")]

use std::sync::Arc;
use std::time::Duration;

use darkburn_gcodebots as gc;
use darkburn_gcodebots::profile as gp;
use darkburn_grbl as grbl;
use darkburn_job::Corner;
use darkburn_job::Toolpath;
use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};

use crate::devices::{ConnectionKind, Device, DeviceSettings, DeviceType};
use crate::job_build::{BuiltJob, DeviceGcodeSettings, Transfer};
use crate::machine_api::{
    ConnectConfig, ConsoleEntry, ConsoleKind, DriverAdapter, Family, Finish, JobData, JobProgress, LaserState, LinkState, MachineCommand,
    MachineError, MachineEvent, Snapshot,
};

/// Which in-memory machine a simulator link runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimGcode {
    /// GRBL through the library's `GrblAdapter` and the GRBL emulator.
    Grbl,
    Smoothie,
    Marlin,
    Snapmaker,
    Custom,
}

impl SimGcode {
    pub const ALL: [SimGcode; 5] = [SimGcode::Grbl, SimGcode::Smoothie, SimGcode::Marlin, SimGcode::Snapmaker, SimGcode::Custom];
    pub fn label(self) -> &'static str {
        match self {
            SimGcode::Grbl => "GRBL, library driver",
            SimGcode::Smoothie => "Smoothieware",
            SimGcode::Marlin => "Marlin",
            SimGcode::Snapmaker => "Snapmaker",
            SimGcode::Custom => "custom G-code",
        }
    }
    fn kind(self) -> gp::DeviceKind {
        match self {
            SimGcode::Grbl => gp::DeviceKind::Grbl,
            SimGcode::Smoothie => gp::DeviceKind::Smoothie,
            SimGcode::Marlin => gp::DeviceKind::Marlin,
            SimGcode::Snapmaker => gp::DeviceKind::Snapmaker,
            SimGcode::Custom => gp::DeviceKind::Custom,
        }
    }
}

// ============================================================================================
// What the profile needs beyond the job builder's G-code settings
// ============================================================================================

/// The library profile's G-code fields that the app's `DeviceGcodeSettings` has no place for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GcodeExt {
    /// Air assist commands, empty for none [DS-12].
    pub air_on: String,
    pub air_off: String,
    /// How a Marlin-style controller is told the power [GCODE-02].
    pub marlin_power: gp::MarlinPowerMode,
    /// Fan or tool index for `M106 P<n>`.
    pub tool_index: u8,
    /// Merge raster moves into longer ones (Smoothieware) [GCODE-01].
    pub clustering: bool,
    pub reduce_smoothie_pause_delay: bool,
    pub max_fire_seconds: f64,
    /// Constant power (LinuxCNC: spindle style output).
    pub constant_power: bool,
    pub custom: gp::CustomTemplates,
    pub snapmaker: gp::SnapmakerSettings,
}

impl Default for GcodeExt {
    fn default() -> Self {
        Self::from_profile(&gc::DeviceProfile::default())
    }
}

impl GcodeExt {
    pub fn from_profile(p: &gc::DeviceProfile) -> Self {
        let g = &p.gcode;
        GcodeExt {
            air_on: g.air_assist.on.clone(),
            air_off: g.air_assist.off.clone(),
            marlin_power: g.marlin_power,
            tool_index: g.tool_index,
            clustering: g.clustering,
            reduce_smoothie_pause_delay: g.reduce_smoothie_pause_delay,
            max_fire_seconds: g.max_fire_seconds,
            constant_power: g.constant_power,
            custom: g.custom.clone(),
            snapmaker: g.snapmaker.clone(),
        }
    }
}

/// Fill in the usual values of a G-code firmware when a device of that type is made (what the
/// New Device wizard starts from).
pub fn apply_preset(kind: DeviceType, s: &mut DeviceSettings) {
    let Some(k) = kind.lib_kind() else { return };
    let p = gc::DeviceProfile::for_kind(k);
    let g = &mut s.gcode;
    g.s_max = p.gcode.s_max;
    g.baud_rate = p.gcode.baud;
    g.start_script = p.gcode.start_script.clone();
    g.end_script = p.gcode.end_script.clone();
    g.transfer = if p.gcode.transfer == gp::TransferMode::Synchronous { Transfer::Synchronous } else { Transfer::Buffered };
    g.jog_with_dollar_j = p.gcode.use_jog_command;
    g.fast_whitespace_scan = p.gcode.fast_whitespace_scan;
    g.fast_whitespace_speed = p.gcode.whitespace_speed / 60.0;
    s.gcode_ext = GcodeExt::from_profile(&p);
}

// ============================================================================================
// Device <-> library profile
// ============================================================================================

fn profile_kind(t: DeviceType) -> gp::DeviceKind {
    t.lib_kind().unwrap_or(match t {
        DeviceType::GrblHal => gp::DeviceKind::GrblHal,
        DeviceType::FluidNc => gp::DeviceKind::FluidNc,
        DeviceType::Grbl | DeviceType::GrblM3 | DeviceType::GrblLpc | DeviceType::Emblaser1 | DeviceType::SimulatorGrbl => gp::DeviceKind::Grbl,
        _ => gp::DeviceKind::DesignOnly,
    })
}

fn device_type(k: gp::DeviceKind) -> DeviceType {
    match k {
        gp::DeviceKind::Grbl => DeviceType::Grbl,
        gp::DeviceKind::GrblHal => DeviceType::GrblHal,
        gp::DeviceKind::FluidNc => DeviceType::FluidNc,
        gp::DeviceKind::Smoothie => DeviceType::Smoothie,
        gp::DeviceKind::Marlin => DeviceType::Marlin,
        gp::DeviceKind::LinuxCnc => DeviceType::LinuxCnc,
        gp::DeviceKind::Snapmaker => DeviceType::Snapmaker,
        gp::DeviceKind::Custom => DeviceType::CustomGcode,
        gp::DeviceKind::DesignOnly => DeviceType::DesignOnly,
    }
}

fn corner_to(c: Corner) -> gp::OriginCorner {
    match c {
        Corner::BottomLeft => gp::OriginCorner::FrontLeft,
        Corner::BottomRight => gp::OriginCorner::FrontRight,
        Corner::TopLeft => gp::OriginCorner::BackLeft,
        Corner::TopRight => gp::OriginCorner::BackRight,
    }
}

fn corner_from(c: gp::OriginCorner) -> Corner {
    match c {
        gp::OriginCorner::FrontLeft | gp::OriginCorner::Center => Corner::BottomLeft,
        gp::OriginCorner::FrontRight => Corner::BottomRight,
        gp::OriginCorner::BackLeft => Corner::TopLeft,
        gp::OriginCorner::BackRight => Corner::TopRight,
    }
}

/// The library profile of a device: every Device Settings field the G-code drivers use.
pub fn to_profile(d: &Device) -> gc::DeviceProfile {
    let s = &d.settings;
    let g = &s.gcode;
    let e = &s.gcode_ext;
    let mut p = gc::DeviceProfile::for_kind(profile_kind(d.kind));
    p.name = d.name.clone();
    p.kind = profile_kind(d.kind);
    p.connection = match d.connection {
        ConnectionKind::Serial => gp::Connection::Serial { port: d.state.port.clone(), baud: g.baud_rate },
        ConnectionKind::Tcp => gp::Connection::Tcp { host: d.host.clone(), port: d.tcp_port },
        _ => gp::Connection::None,
    };
    p.width_mm = d.width_mm;
    p.height_mm = d.height_mm;
    p.origin = corner_to(d.origin);
    p.negative_coordinates = s.negative_coords;
    p.pointer_offset = gp::PointerOffset { x: s.red_dot_x, y: s.red_dot_y };
    p.z = gp::ZSettings {
        enabled: s.z_enabled,
        reverse: s.z_reverse,
        relative_moves_only: s.gcode.z_relative_only,
        optimize_moves: s.gcode.z_optimize,
    };
    p.tab_pulse_width_mm = s.tab_pulse_width;
    p.checklist = gp::JobChecklist { enabled: s.checklist_on, items: s.checklist.clone() };
    p.overwrite_files = s.overwrite_files;
    p.scanning_offsets = gp::ScanningOffsets {
        enabled: !s.scan.offsets.entries.is_empty(),
        table: s.scan.offsets.entries.iter().map(|o| gp::ScanOffset { speed_mm_s: o.speed, offset_mm: o.offset }).collect(),
    };
    p.gcode = gp::GcodeSettings {
        auto_home_on_start: g.auto_home,
        swap_xy: g.swap_xy,
        fast_whitespace_scan: g.fast_whitespace_scan,
        whitespace_speed: g.fast_whitespace_speed * 60.0,
        use_jog_command: g.jog_with_dollar_j,
        enable_dtr: g.dtr,
        fire_button: g.fire_button,
        laser_on_when_framing: g.laser_on_when_framing,
        out_of_bounds_warning: true,
        clustering: e.clustering,
        return_to_finish_position: g.return_to_finish,
        air_assist: gp::AirAssist { on: e.air_on.clone(), off: e.air_off.clone() },
        s_max: g.s_max,
        baud: g.baud_rate,
        transfer: if g.transfer == Transfer::Synchronous { gp::TransferMode::Synchronous } else { gp::TransferMode::Buffered },
        start_script: g.start_script.clone(),
        end_script: g.end_script.clone(),
        g0_for_overscan: g.overscan_as_g0,
        s_with_every_g1: g.s_every_g1,
        constant_power: e.constant_power,
        // The job is always written in millimetres. Inches on the screen (`state.inches`) is how
        // sizes are shown, and goes to `state.units` below.
        units: gp::Units::Millimetres,
        marlin_power: e.marlin_power,
        tool_index: e.tool_index,
        reduce_smoothie_pause_delay: e.reduce_smoothie_pause_delay,
        max_fire_percent: s.fire_max_percent,
        max_fire_seconds: e.max_fire_seconds,
        custom: e.custom.clone(),
        snapmaker: e.snapmaker.clone(),
    };
    let a = &s.additional;
    p.simulation =
        gp::SimulationSettings { accel_x: a.accel, accel_y: a.accel, max_speed_x: a.max_speed, max_speed_y: a.max_speed, rapid_speed: a.rapid_speed };
    p.state.units = if d.state.inches { gp::Units::Inches } else { gp::Units::Millimetres };
    p.state.grid_spacing_mm = d.state.grid_spacing;
    p.state.last_port = (!d.state.port.is_empty()).then(|| d.state.port.clone());
    p
}

/// A device made from a library profile (the other direction of [`to_profile`]).
pub fn from_profile(p: &gc::DeviceProfile) -> Device {
    let mut d = Device::new(device_type(p.kind));
    d.name = p.name.clone();
    match &p.connection {
        gp::Connection::Serial { port, baud } => {
            d.connection = ConnectionKind::Serial;
            d.state.port = port.clone();
            d.settings.gcode.baud_rate = *baud;
        }
        gp::Connection::Tcp { host, port } => {
            d.connection = ConnectionKind::Tcp;
            d.host = host.clone();
            d.tcp_port = *port;
        }
        gp::Connection::None => {}
    }
    d.width_mm = p.width_mm;
    d.height_mm = p.height_mm;
    d.origin = corner_from(p.origin);
    let s = &mut d.settings;
    s.negative_coords = p.negative_coordinates;
    s.red_dot_x = p.pointer_offset.x;
    s.red_dot_y = p.pointer_offset.y;
    s.z_enabled = p.z.enabled;
    s.z_reverse = p.z.reverse;
    s.gcode.z_relative_only = p.z.relative_moves_only;
    s.gcode.z_optimize = p.z.optimize_moves;
    s.tab_pulse_width = p.tab_pulse_width_mm;
    s.checklist_on = p.checklist.enabled;
    s.checklist = p.checklist.items.clone();
    s.overwrite_files = p.overwrite_files;
    s.scan.offsets.entries =
        p.scanning_offsets.table.iter().map(|o| crate::scan_offsets::OffsetEntry { speed: o.speed_mm_s, offset: o.offset_mm }).collect();
    s.fire_max_percent = p.gcode.max_fire_percent;
    let g = &mut s.gcode;
    let pg = &p.gcode;
    g.auto_home = pg.auto_home_on_start;
    g.swap_xy = pg.swap_xy;
    g.fast_whitespace_scan = pg.fast_whitespace_scan;
    g.fast_whitespace_speed = pg.whitespace_speed / 60.0;
    g.jog_with_dollar_j = pg.use_jog_command;
    g.dtr = pg.enable_dtr;
    g.fire_button = pg.fire_button;
    g.laser_on_when_framing = pg.laser_on_when_framing;
    g.return_to_finish = pg.return_to_finish_position;
    g.s_max = pg.s_max;
    g.baud_rate = pg.baud;
    g.transfer = if pg.transfer == gp::TransferMode::Synchronous { Transfer::Synchronous } else { Transfer::Buffered };
    g.start_script = pg.start_script.clone();
    g.end_script = pg.end_script.clone();
    g.overscan_as_g0 = pg.g0_for_overscan;
    g.s_every_g1 = pg.s_with_every_g1;
    s.gcode_ext = GcodeExt::from_profile(p);
    s.additional.accel = p.simulation.accel_x;
    s.additional.max_speed = p.simulation.max_speed_x;
    s.additional.rapid_speed = p.simulation.rapid_speed;
    d.state.inches = p.state.units == gp::Units::Inches;
    d.state.grid_spacing = p.state.grid_spacing_mm;
    d
}

/// A device as the library's own JSON device file (DEV-05, for use with other tools).
pub fn export_library_profile(d: &Device) -> String {
    to_profile(d).export_json()
}

/// Devices from a library JSON device file.
pub fn import_library_profiles(text: &str) -> Result<Vec<Device>, String> {
    gc::DeviceProfile::import_many(text).map(|v| v.iter().map(from_profile).collect()).map_err(|e| e.to_string())
}

// ============================================================================================
// Jobs: neutral G-code from the job builder, translated to the device's dialect
// ============================================================================================

/// What the job builder writes for a translated device: plain GRBL-style G-code. The device's
/// scripts, homing, S range and air commands are applied by the translation instead.
pub fn neutral_gcode(base: &DeviceGcodeSettings) -> DeviceGcodeSettings {
    DeviceGcodeSettings {
        flavor: darkburn_job::gcode::Flavor::Grbl,
        s_max: 1000.0,
        air: darkburn_job::gcode::AirCommand::M8,
        auto_home: false,
        start_script: String::new(),
        end_script: String::new(),
        overscan_as_g0: false,
        s_every_g1: false,
        tool_index: None,
        marlin_power: darkburn_job::gcode::MarlinPower::M3,
        ..base.clone()
    }
}

/// Translate GRBL-style lines for `dev`. LinuxCNC gets a complete `.ngc` program.
pub fn translate_for(dev: &Device, lines: &[String]) -> Vec<String> {
    let p = to_profile(dev);
    // The job builder ends its GRBL-style text with `M2`. The device writes its own end after
    // it (end script, laser off, and for LinuxCNC the program end), and a controller that knows
    // `M2` stops reading there: the job's own program end is left out.
    let is_end = |l: &String| l.trim().eq_ignore_ascii_case("M2") || l.trim().eq_ignore_ascii_case("M30");
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    let lines = match last {
        Some(i) if is_end(&lines[i]) => &lines[..i],
        _ => lines,
    };
    if p.kind == gp::DeviceKind::LinuxCnc {
        let o = gc::linuxcnc::LinuxCncOptions::for_profile(&p);
        return gc::linuxcnc::write_program(lines, &p, &o).lines().map(str::to_owned).collect();
    }
    gc::translate::translate(lines, &gc::translate::TranslateOptions::for_profile(&p))
}

/// The lines to stream to `dev` for `tp`. `laser_mode_off`: the connected GRBL controller says
/// its laser mode is off (`$32=0`), so it keeps the laser on through rapid moves like an old
/// one does, and the travel moves switch it off themselves.
pub fn job_lines(dev: &Device, tp: &Toolpath, base: &DeviceGcodeSettings, laser_mode_off: bool) -> Vec<String> {
    if !dev.kind.is_translated() {
        return crate::job_build::gcode_lines_for(tp, base, dev.m3_only() || laser_mode_off);
    }
    let neutral = crate::job_build::gcode_lines(tp, &neutral_gcode(base));
    translate_for(dev, &neutral)
}

/// The text of a G-code file for `dev` (Save G-code). Snapmaker files get their header.
pub fn job_text(dev: &Device, job: &BuiltJob, from_op: Option<usize>, base: &DeviceGcodeSettings) -> String {
    if !dev.kind.is_translated() {
        return crate::job_build::gcode_text_for(job, from_op, base, dev.m3_only());
    }
    let neutral = crate::job_build::gcode_text(job, from_op, &neutral_gcode(base));
    let lines: Vec<String> = neutral.lines().map(str::to_owned).collect();
    let mut out = translate_for(dev, &lines);
    // The translation drops comments, and with them the line that says the file is DarkBurn's
    // (Run G-code reads it to know the file holds bed positions). A LinuxCNC program has its
    // own title line.
    if dev.kind.lib_kind() != Some(gp::DeviceKind::LinuxCnc) {
        out.insert(0, format!("; DarkBurn job for {}", dev.kind.label()));
    }
    if dev.kind.lib_kind() == Some(gp::DeviceKind::Snapmaker) {
        let p = to_profile(dev);
        let info = gc::snapmaker::JobInfo { bounds: job.bounds, work_speed: 0.0, estimated_seconds: 0.0, power_percent: 0.0 };
        return gc::snapmaker::file_text(&p, &info, &out);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// A G-code file for `dev` with the move to the Move panel's finish position (a machine
/// position) at its end, as a started job gets it: after the laser is off and the end script,
/// before the line that ends the program.
pub fn with_finish_move(dev: &Device, text: String, finish: Option<(f64, f64)>) -> String {
    let Some((x, y)) = finish else { return text };
    let num = |v: f64| {
        let s = format!("{:.3}", if v.abs() < 0.0005 { 0.0 } else { v });
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    let go: Vec<String> = if dev.kind.is_translated() {
        vec!["G90".into(), format!("G0 X{} Y{}", num(x), num(y))]
    } else {
        vec![format!("G90 G53 G0 X{} Y{}", num(x), num(y))]
    };
    let mut lines: Vec<&str> = text.lines().collect();
    // A controller stops reading at the first program end, so the move goes before that one.
    let program_end = lines.iter().position(|l| l.trim().eq_ignore_ascii_case("M2") || l.trim().eq_ignore_ascii_case("M30"));
    let at = program_end.unwrap_or_else(|| {
        let mut at = lines.len();
        while at > 0 && (lines[at - 1].trim().is_empty() || lines[at - 1].trim() == "%") {
            at -= 1;
        }
        at
    });
    for (i, l) in go.iter().enumerate() {
        lines.insert(at + i, l);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The start and end scripts the device writes around a job, as the job file shows them
/// [DS-14].
pub fn scripts_of(dev: &Device) -> (Vec<String>, Vec<String>) {
    let s = &dev.settings.gcode;
    let lines = |t: &str| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    (lines(&s.start_script), lines(&s.end_script))
}

// ============================================================================================
// DEV-02: firmware guess from a banner
// ============================================================================================

/// The device type a firmware banner or version string looks like.
pub fn guess_type(banner: &str) -> Option<DeviceType> {
    use gc::FirmwareGuess as F;
    Some(match gc::identify(banner).firmware {
        F::Grbl => DeviceType::Grbl,
        F::GrblHal => DeviceType::GrblHal,
        F::FluidNc => DeviceType::FluidNc,
        F::Smoothie => DeviceType::Smoothie,
        F::Marlin => DeviceType::Marlin,
        F::Snapmaker => DeviceType::Snapmaker,
        F::Unknown => return None,
    })
}

/// Whether a device of type `t` is a sensible home for firmware that looks like `guess`.
fn compatible(t: DeviceType, guess: DeviceType) -> bool {
    let grbl = |x: DeviceType| {
        matches!(
            x,
            DeviceType::Grbl
                | DeviceType::GrblM3
                | DeviceType::GrblLpc
                | DeviceType::GrblHal
                | DeviceType::FluidNc
                | DeviceType::Emblaser1
                | DeviceType::SimulatorGrbl
        )
    };
    let smoothie = |x: DeviceType| matches!(x, DeviceType::Smoothie | DeviceType::Emblaser2 | DeviceType::SimulatorSmoothie);
    match t {
        DeviceType::CustomGcode | DeviceType::SimulatorCustom => true,
        DeviceType::Marlin | DeviceType::Snapmaker | DeviceType::SimulatorMarlin | DeviceType::SimulatorSnapmaker => {
            matches!(guess, DeviceType::Marlin | DeviceType::Snapmaker)
        }
        _ if grbl(t) => grbl(guess),
        _ if smoothie(t) => smoothie(guess),
        _ => true,
    }
}

/// A sentence for the connect message when the firmware that answered is not the type the user
/// chose. Empty when it fits or cannot be told.
pub fn mismatch_hint(firmware: &str, chosen: DeviceType) -> String {
    match guess_type(firmware) {
        Some(g) if !compatible(chosen, g) => {
            format!(". This looks like {}, but the device is set to {}. Change the type in Device Settings.", g.label(), chosen.label())
        }
        // GRBL before 1.1f has no dynamic power (M4): the GRBL-M3 type writes M3 only [GRBL-02].
        _ if matches!(chosen, DeviceType::Grbl | DeviceType::Emblaser1) && crate::connections::grbl_is_m3_only(firmware) => {
            ". This GRBL is older than 1.1f and has constant power only. Change the type to GRBL-M3 in Device Settings.".into()
        }
        _ => String::new(),
    }
}

// ============================================================================================
// The adapter
// ============================================================================================

enum SimHold {
    Line(#[allow(dead_code)] gc::LineEmuHandle),
    Grbl(#[allow(dead_code)] grbl::EmulatorHandle),
}

pub struct GcodeAdapter {
    m: Option<Box<dyn gc::GcodeMachine>>,
    profile: Arc<gc::DeviceProfile>,
    simulator: bool,
    _sim: Option<SimHold>,
    snap: Snapshot,
    estimated: Option<Duration>,
    out: Vec<MachineEvent>,
}

fn map_err(e: gc::MachineError) -> MachineError {
    use gc::MachineError as G;
    match e {
        G::NotConnected | G::NotReady => MachineError::NotConnected,
        G::Busy => MachineError::Busy,
        G::Alarm => MachineError::Alarm,
        G::NotIdle(s) => MachineError::Driver(format!("The laser must be idle (it is {s}).")),
        G::OutOfBounds(m) => MachineError::OutOfBounds(m),
        G::Unsupported(m) => MachineError::Unsupported(m.to_owned()),
        G::InvalidArgument(m) => MachineError::Invalid(m),
    }
}

fn console_kind(k: grbl::ConsoleKind) -> ConsoleKind {
    match k {
        grbl::ConsoleKind::Sent => ConsoleKind::Sent,
        grbl::ConsoleKind::Received => ConsoleKind::Received,
        grbl::ConsoleKind::Note => ConsoleKind::Note,
    }
}

impl GcodeAdapter {
    pub fn new(machine: Box<dyn gc::GcodeMachine>, profile: Arc<gc::DeviceProfile>, simulator: bool) -> Self {
        let bed = Some((profile.width_mm, profile.height_mm));
        let snap = Snapshot {
            family: Family::Grbl,
            simulator,
            link: LinkState::Connecting,
            state: LaserState::Connecting,
            bed,
            gcode_firmware: Some(machine.family()),
            ..Default::default()
        };
        Self { m: Some(machine), profile, simulator, _sim: None, snap, estimated: None, out: Vec::new() }
    }

    fn with_sim(mut self, s: SimHold) -> Self {
        self._sim = Some(s);
        self
    }

    fn machine(&self) -> Result<&dyn gc::GcodeMachine, MachineError> {
        self.m.as_deref().ok_or(MachineError::NotConnected)
    }

    fn progress(&self, p: &grbl::JobProgress) -> JobProgress {
        let fraction = p.fraction();
        let remaining = p.eta.or_else(|| {
            self.estimated.map(|e| e.saturating_sub(p.elapsed)).or_else(|| fraction.filter(|f| *f > 0.02).map(|f| p.elapsed.mul_f64((1.0 - f) / f)))
        });
        JobProgress {
            fraction,
            elapsed: p.elapsed,
            remaining,
            lines_done: p.lines_acked,
            total_lines: p.total_lines,
            errors: p.errors,
            framing: p.kind == grbl::JobKind::Frame,
        }
    }

    fn refresh(&mut self) {
        let Some(m) = &self.m else { return };
        let g = m.snapshot();
        let job = g.job.as_ref().map(|p| self.progress(p));
        let s = &mut self.snap;
        s.link = match &g.connection {
            grbl::ConnectionState::Disconnected => LinkState::Disconnected,
            grbl::ConnectionState::Connecting => LinkState::Connecting,
            grbl::ConnectionState::Connected => LinkState::Connected,
            grbl::ConnectionState::Failed(e) => LinkState::Failed(e.clone()),
        };
        s.state = match g.laser_status() {
            grbl::LaserStatus::Disconnected => LaserState::Disconnected,
            grbl::LaserStatus::Connecting => LaserState::Connecting,
            grbl::LaserStatus::Ready => LaserState::Ready,
            grbl::LaserStatus::Busy => LaserState::Busy,
            grbl::LaserStatus::Paused => LaserState::Paused,
            grbl::LaserStatus::Framing => LaserState::Framing,
            grbl::LaserStatus::Alarm => LaserState::Alarm,
        };
        s.port = g.port.clone();
        s.firmware = g.firmware.clone();
        s.has_position = g.updated.is_some();
        s.machine_pos = [g.mpos[0], g.mpos[1], g.mpos[2], 0.0];
        s.work_pos = [g.wpos[0], g.wpos[1], g.wpos[2], 0.0];
        s.fire_on = g.fire_on;
        s.air_on = g.air_on;
        s.alarm = g.alarm.clone();
        s.gcode_settings = g.settings.clone();
        s.gcode_firmware = g.family.or(s.gcode_firmware);
        // Progress only while something runs; the finished job is reported by its event.
        s.job = job.filter(|_| s.state.is_working());
    }
}

impl DriverAdapter for GcodeAdapter {
    fn poll(&mut self) -> Vec<MachineEvent> {
        let Some(m) = &self.m else { return Vec::new() };
        let events = m.poll_events();
        for ev in events {
            use gc::MachineEvent as E;
            match ev {
                E::Connected(fw) => self.out.push(MachineEvent::Connected(fw)),
                E::ConnectFailed(e) => self.out.push(MachineEvent::ConnectFailed(e)),
                E::Disconnected(r) => self.out.push(MachineEvent::Disconnected(r)),
                E::StateChanged(_) | E::JobProgress(_) | E::FireOff => {}
                E::Console(l) => self.out.push(MachineEvent::Console(ConsoleEntry { kind: console_kind(l.kind), text: l.text })),
                E::Alarm(m) => self.out.push(MachineEvent::Alarm(m)),
                E::Error { message, line } => self.out.push(MachineEvent::Error(format!("{message} (line: {line})"))),
                E::Settings(_) => self.out.push(MachineEvent::SettingsLoaded),
                E::SettingWritten { key, value, result } => self.out.push(MachineEvent::SettingWritten {
                    id: key.clone(),
                    ok: result.is_ok(),
                    message: result.err().unwrap_or_else(|| format!("{key}={value}")),
                }),
                E::JobStarted => self.out.push(MachineEvent::JobStarted),
                E::JobFinished(p) => {
                    let p = self.progress(&p);
                    self.out.push(MachineEvent::JobFinished(p));
                }
                E::JobLineError { line_number, line, message } => {
                    self.out.push(MachineEvent::Error(format!("Line {line_number} ({line}): {message}")))
                }
                E::JobAborted { reason, .. } => self.out.push(MachineEvent::JobAborted(reason)),
                E::Stopped => self.out.push(MachineEvent::Stopped),
                E::Homed => self.out.push(MachineEvent::Homed),
                E::Rejected(m) => self.out.push(MachineEvent::Error(m)),
            }
        }
        self.refresh();
        std::mem::take(&mut self.out)
    }

    fn snapshot(&self) -> Snapshot {
        self.snap.clone()
    }

    fn command(&mut self, cmd: MachineCommand) -> Result<(), MachineError> {
        use MachineCommand as C;
        let m = self.machine()?;
        let unsupported = |what: &str| Err(MachineError::Unsupported(format!("{what} is not available on this controller")));
        match cmd {
            C::StartJob(req) => {
                let JobData::Gcode(lines) = req.data else {
                    return Err(MachineError::Unsupported("a G-code machine runs G-code, not controller files".into()));
                };
                let n = lines.len();
                let mut job = grbl::Job::from_lines(lines).with_total(n).with_stop_on_error(req.stop_on_error);
                if let Some(b) = req.bounds {
                    job = job.with_bounds(grbl::BoundsCheck { job: b, machine: self.profile.travel_limits().xy });
                }
                job = job.with_finish(match req.finish {
                    Finish::Stay => grbl::FinishPosition::Stay,
                    Finish::Origin => grbl::FinishPosition::Origin,
                    Finish::Machine(p) => grbl::FinishPosition::Machine(p),
                });
                self.estimated = req.estimated;
                self.machine()?.start_job(job).map_err(map_err)
            }
            C::Pause => m.pause().map_err(map_err),
            C::Resume => m.resume().map_err(map_err),
            C::Stop => m.stop().map_err(map_err),
            C::EmergencyStop => m.emergency_stop().map_err(map_err),
            C::FrameRect(r, f) => {
                m.frame_rect(r, grbl::FrameOptions { feed: f.feed, laser_percent: f.laser_percent, return_to: f.return_to }).map_err(map_err)
            }
            C::FrameOutline(p, f) => {
                m.frame_outline(&p, grbl::FrameOptions { feed: f.feed, laser_percent: f.laser_percent, return_to: f.return_to }).map_err(map_err)
            }
            C::FrameOnController => unsupported("Framing on the controller"),
            C::JogStep(j) => {
                if j.du != 0.0 {
                    return Err(MachineError::Unsupported("this controller has no rotary axis".into()));
                }
                m.jog(grbl::JogStep { dx: j.dx, dy: j.dy, dz: j.dz, feed: j.feed }).map_err(map_err)
            }
            C::JogStart { .. } => unsupported("Continuous jogging (use the step buttons)"),
            C::JogStop => m.jog_cancel().map_err(map_err),
            C::Home => m.home().map_err(map_err),
            C::FocusZ => unsupported("A focus routine"),
            C::Unlock => m.unlock().map_err(map_err),
            C::GoToOrigin => m.go_to_origin().map_err(map_err),
            C::MoveTo(t) => m
                .move_to(grbl::MoveTarget {
                    x: t.x,
                    y: t.y,
                    z: t.z,
                    coords: if t.machine_coords { grbl::Coords::Machine } else { grbl::Coords::Work },
                    feed: t.feed,
                })
                .map_err(map_err),
            C::SetUserOrigin => m.set_user_origin().map_err(map_err),
            C::ClearUserOrigin => m.clear_user_origin().map_err(map_err),
            C::GetPosition => m.request_status().map_err(map_err),
            C::Fire { percent, secs } => m.fire(percent, crate::machine_api::fire_time(secs)?).map_err(map_err),
            C::FireOff => m.fire_off().map_err(map_err),
            C::Air(on) => m.set_air(on).map_err(map_err),
            C::SetOverrides { feed, power, rapid } => {
                if rapid.is_some() {
                    return unsupported("The rapid override");
                }
                if let Some(f) = feed {
                    m.set_feed_override(f).map_err(map_err)?;
                }
                if let Some(p) = power {
                    m.set_power_override(p).map_err(map_err)?;
                }
                Ok(())
            }
            C::Console(l) => m.send_console(&l).map_err(map_err),
            C::Macro(t) => m.run_macro(&t).map_err(map_err),
            C::ShowAll(_) => Ok(()),
            C::ReadSettings => m.read_settings().map_err(map_err),
            C::WriteGcodeSetting { key, value } => m.write_setting(&key, &value).map_err(map_err),
            C::PersistSettings => m.persist_settings().map_err(map_err),
            C::WriteSetting { .. } => Err(MachineError::Invalid("Settings of this controller are written by name, not by number.".into())),
            C::RestoreSettings(_) => unsupported("Restoring a settings backup"),
            C::ListFiles | C::RunFile(_) | C::DeleteFiles(_) | C::DeleteAllFiles | C::CalcTime(_) | C::AnswerOverwrite(_) => {
                unsupported("The controller file list")
            }
            C::ReadControllerInfo => unsupported("Controller info"),
            C::ReadLaser2 | C::WriteLaser2(_) => unsupported("Laser 2 settings"),
            C::ReadUAxis | C::WriteUAxis(_) => unsupported("U axis settings"),
            C::GalvoArm(_) | C::GalvoFrame { .. } | C::GalvoLight(_) | C::GalvoLens(_) | C::GalvoAxis { .. } => {
                Err(MachineError::Unsupported("that is a galvo command; this is a G-code machine".into()))
            }
        }
    }

    fn close(&mut self) {
        if let Some(m) = self.m.take() {
            m.disconnect();
        }
        self.snap.link = LinkState::Disconnected;
        self.snap.state = LaserState::Disconnected;
        let _ = self.simulator;
    }
}

impl Drop for GcodeAdapter {
    fn drop(&mut self) {
        self.close();
    }
}

// ============================================================================================
// Opening
// ============================================================================================

/// The adapter for a real transport, chosen by the device profile in `cfg`: the line-paced
/// firmwares of `darkburn-gcodebots`, or the built-in GRBL driver. Called only from
/// `machine_api::open_real`, after the user's Connect click.
pub fn adapter_for(transport: Box<dyn grbl::Transport>, cfg: &ConnectConfig) -> Box<dyn DriverAdapter> {
    match &cfg.gcode {
        Some(p) if uses_library_driver(cfg) => match p.connect(transport) {
            Ok(m) => Box::new(GcodeAdapter::new(m, p.clone(), false)),
            Err(e) => Box::new(FailedAdapter(e.to_string())),
        },
        _ => Box::new(crate::machine_api::GrblAdapter::new(transport, cfg, false, None)),
    }
}

/// Stands in when a profile has no live driver; reports the reason and does nothing.
struct FailedAdapter(String);

impl DriverAdapter for FailedAdapter {
    fn poll(&mut self) -> Vec<MachineEvent> {
        vec![MachineEvent::ConnectFailed(self.0.clone())]
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot { link: LinkState::Failed(self.0.clone()), ..Default::default() }
    }
    fn command(&mut self, _cmd: MachineCommand) -> Result<(), MachineError> {
        Err(MachineError::NotConnected)
    }
    fn close(&mut self) {}
}

/// Whether a real link for `cfg` goes to the library driver (otherwise the built-in GRBL one).
pub fn uses_library_driver(cfg: &ConnectConfig) -> bool {
    cfg.gcode
        .as_ref()
        .is_some_and(|p| p.kind.has_live_driver() && !matches!(p.kind, gp::DeviceKind::Grbl | gp::DeviceKind::GrblHal | gp::DeviceKind::FluidNc))
}

/// An in-memory machine for tests and demos: the adapter and the emulator handle.
pub fn simulator_adapter(kind: SimGcode, cfg: &ConnectConfig) -> (GcodeAdapter, Option<gc::LineEmuHandle>) {
    let mut profile = cfg.gcode.as_deref().cloned().unwrap_or_else(|| gc::DeviceProfile::for_kind(kind.kind()));
    if profile.kind != kind.kind() {
        let keep = (profile.name.clone(), profile.width_mm, profile.height_mm);
        profile = gc::DeviceProfile::for_kind(kind.kind());
        (profile.name, profile.width_mm, profile.height_mm) = keep;
    }
    if let Some((w, h)) = cfg.bed {
        profile.width_mm = w;
        profile.height_mm = h;
    }
    let scale = cfg.sim_time_scale.max(0.1);
    let profile = Arc::new(profile);
    match kind {
        SimGcode::Grbl => {
            let mut ecfg = grbl::EmulatorConfig::atomstack().with_time_scale(scale);
            ecfg.record_lines = true;
            if cfg.sim_homing {
                ecfg = ecfg.with_setting(22, 1.0);
            }
            if let Some((w, h)) = cfg.bed {
                ecfg = ecfg.with_setting(130, w).with_setting(131, h);
            }
            let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(ecfg));
            let mut opts = gc::grbl_adapter::connect_options(&profile);
            opts.banner_timeout = Duration::from_millis(800);
            let m = gc::GrblAdapter::with_options(Box::new(emu.transport()), opts, &profile.gcode.air_assist.on, &profile.gcode.air_assist.off);
            (GcodeAdapter::new(Box::new(m), profile, true).with_sim(SimHold::Grbl(emu)), None)
        }
        _ => {
            let flavor = match kind {
                SimGcode::Smoothie => gc::EmuFlavor::Smoothie,
                SimGcode::Marlin => gc::EmuFlavor::Marlin,
                SimGcode::Snapmaker => gc::EmuFlavor::Snapmaker,
                _ => gc::EmuFlavor::Plain,
            };
            let mut ecfg = gc::EmuConfig::new(flavor).with_time_scale(scale);
            ecfg.travel = [profile.width_mm.max(1.0), profile.height_mm.max(1.0), 50.0];
            if matches!(kind, SimGcode::Marlin | SimGcode::Snapmaker) && profile.gcode.marlin_power == gp::MarlinPowerMode::Fan {
                ecfg = ecfg.with_laser_fan(profile.gcode.tool_index);
            }
            let emu = gc::LineEmuHandle::with_config(ecfg);
            let machine = profile.connect(Box::new(emu.transport())).expect("the line firmwares always connect");
            (GcodeAdapter::new(machine, profile, true).with_sim(SimHold::Line(emu.clone())), Some(emu))
        }
    }
}

/// The simulator link of `machine_api`.
pub fn open_simulator(kind: SimGcode, cfg: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    Ok(Box::new(simulator_adapter(kind, cfg).0))
}

/// A square job in the dialect of `dev`, for the simulators (no job builder needed).
pub fn simulator_square_lines(dev: &Device, size: f64) -> Vec<String> {
    let s = size;
    let lines = vec![
        "G21 G90".to_owned(),
        "M4 S300".to_owned(),
        "G0 X10 Y10".to_owned(),
        format!("G1 X{} Y10 F6000", 10.0 + s),
        format!("G1 X{} Y{}", 10.0 + s, 10.0 + s),
        format!("G1 X10 Y{}", 10.0 + s),
        "G1 X10 Y10".to_owned(),
        "M5".to_owned(),
    ];
    if dev.kind.is_translated() { translate_for(dev, &lines) } else { lines }
}

/// The `Rect` a G-code machine can reach (for bounds checks in tests and the UI).
pub fn travel_rect(dev: &Device) -> Rect {
    to_profile(dev).travel_limits().xy
}

/// The firmware names the Machine Settings page can show for a controller.
pub fn firmware_label(f: gc::MachineFamily) -> &'static str {
    f.label()
}

/// A point helper used by tests.
pub fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}
