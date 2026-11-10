//! Device profiles: the data behind the Devices dialog, the New Device wizard and the Device
//! Settings dialog [DEV-01..DEV-08, DS-01..DS-17, DS-19]. Nothing here touches hardware: Find My
//! Laser only reads the port list it is given, and a profile only becomes a connection when the
//! user clicks Connect in the app.
//!
//! The G-code and motion settings are the job builder's own structs (`job_build.rs`); a device
//! profile holds them and `machine_ui::sync_job_state` hands them to the job builder.

#![allow(dead_code, reason = "wired into the dialogs and panels in machine_ui.rs and devices_ui.rs")]

use std::path::Path;

use darkburn_grbl::PortInfo;
use darkburn_job::Corner;
use darkburn_job::gcode::Flavor;
use serde::{Deserialize, Serialize};

use crate::gcode_machines::SimGcode;
use crate::job_build::{AdditionalSettings, DeviceGcodeSettings, JobDevice, Transfer};
use crate::machine_api::{ConnectConfig, ConnectSpec, Family, Link};

/// What the user picks first in the wizard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceType {
    #[default]
    Grbl,
    GrblHal,
    FluidNc,
    Ruida,
    SimulatorGrbl,
    SimulatorRuida,
    DesignOnly,
    // G-code controllers of darkburn-gcodebots [GCODE-01..05]
    Smoothie,
    Marlin,
    Snapmaker,
    /// File output only (no live link).
    LinuxCnc,
    Emblaser1,
    Emblaser2,
    CustomGcode,
    SimulatorSmoothie,
    SimulatorMarlin,
    SimulatorSnapmaker,
    SimulatorCustom,
    // Galvo [GALVO-01]
    Galvo,
    SimulatorGalvo,
    // Gweike M3 Ultra: files only, plus the card simulator [MCC-05]
    GweikeCardSimulator,
    // Listed so people can see they are planned: each needs a capture from real hardware first.
    SnapmakerNetwork,
    EmblaserWifi,
    Cubiio,
    Trocen,
    TopWisdom,
}

impl DeviceType {
    pub const ALL: [DeviceType; 26] = [
        DeviceType::Grbl,
        DeviceType::GrblHal,
        DeviceType::FluidNc,
        DeviceType::Smoothie,
        DeviceType::Marlin,
        DeviceType::Snapmaker,
        DeviceType::Emblaser1,
        DeviceType::Emblaser2,
        DeviceType::LinuxCnc,
        DeviceType::CustomGcode,
        DeviceType::Ruida,
        DeviceType::Galvo,
        DeviceType::GweikeCardSimulator,
        DeviceType::SimulatorGrbl,
        DeviceType::SimulatorRuida,
        DeviceType::SimulatorSmoothie,
        DeviceType::SimulatorMarlin,
        DeviceType::SimulatorSnapmaker,
        DeviceType::SimulatorCustom,
        DeviceType::SimulatorGalvo,
        DeviceType::DesignOnly,
        DeviceType::SnapmakerNetwork,
        DeviceType::EmblaserWifi,
        DeviceType::Cubiio,
        DeviceType::Trocen,
        DeviceType::TopWisdom,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Grbl => "GRBL",
            Self::GrblHal => "grblHAL",
            Self::FluidNc => "FluidNC",
            Self::Ruida => "Ruida (DSP)",
            Self::SimulatorGrbl => "Simulator (GRBL)",
            Self::SimulatorRuida => "Simulator (Ruida)",
            Self::DesignOnly => "Design only (no laser)",
            Self::Smoothie => "Smoothieware",
            Self::Marlin => "Marlin",
            Self::Snapmaker => "Snapmaker (USB serial)",
            Self::LinuxCnc => "LinuxCNC (file output)",
            Self::Emblaser1 => "Emblaser 1 (GRBL)",
            Self::Emblaser2 => "Emblaser 2 (serial)",
            Self::CustomGcode => "Custom G-code device",
            Self::SimulatorSmoothie => "Simulator (Smoothieware)",
            Self::SimulatorMarlin => "Simulator (Marlin)",
            Self::SimulatorSnapmaker => "Simulator (Snapmaker)",
            Self::SimulatorCustom => "Simulator (custom G-code)",
            Self::Galvo => "Galvo (EZCad2 board)",
            Self::SimulatorGalvo => "Simulator (galvo)",
            Self::GweikeCardSimulator => "Gweike M3 Ultra: card simulator (files only)",
            Self::SnapmakerNetwork => "Snapmaker over the network",
            Self::EmblaserWifi => "Emblaser 2 over Wi-Fi",
            Self::Cubiio => "Cubiio",
            Self::Trocen => "Trocen / AWC",
            Self::TopWisdom => "TopWisdom",
        }
    }
    pub fn family(self) -> Family {
        match self {
            Self::Grbl
            | Self::GrblHal
            | Self::FluidNc
            | Self::SimulatorGrbl
            | Self::Smoothie
            | Self::Marlin
            | Self::Snapmaker
            | Self::LinuxCnc
            | Self::Emblaser1
            | Self::Emblaser2
            | Self::CustomGcode
            | Self::SimulatorSmoothie
            | Self::SimulatorMarlin
            | Self::SimulatorSnapmaker
            | Self::SimulatorCustom => Family::Grbl,
            Self::Ruida | Self::SimulatorRuida => Family::Ruida,
            Self::Galvo | Self::SimulatorGalvo => Family::Galvo,
            Self::GweikeCardSimulator => Family::FileOnly,
            Self::DesignOnly | Self::SnapmakerNetwork | Self::EmblaserWifi | Self::Cubiio | Self::Trocen | Self::TopWisdom => Family::DesignOnly,
        }
    }
    pub fn is_simulator(self) -> bool {
        matches!(
            self,
            Self::SimulatorGrbl
                | Self::SimulatorRuida
                | Self::SimulatorSmoothie
                | Self::SimulatorMarlin
                | Self::SimulatorSnapmaker
                | Self::SimulatorCustom
                | Self::SimulatorGalvo
                | Self::GweikeCardSimulator
        )
    }
    /// Why this type cannot be used yet: it needs a capture from real hardware first. These
    /// are listed in the wizard, greyed out, with this text.
    pub fn needs_capture(self) -> Option<&'static str> {
        Some(match self {
            Self::SnapmakerNetwork => {
                "Needs hardware capture. Snapmaker's network (HTTP) interface is not documented anywhere DarkBurn may read. Use the USB serial entry for now."
            }
            Self::EmblaserWifi => {
                "Needs hardware capture. The Wi-Fi port and protocol of the Emblaser 2 are not documented. Use the serial entry for now."
            }
            Self::Cubiio => {
                "Needs hardware capture. The Cubiio only takes G-code files over Wi-Fi through an undocumented web interface. Export G-code and load it with Cubiio's own app."
            }
            Self::Trocen => {
                "Needs hardware capture. There is no public description of the Trocen / AWC USB or Ethernet protocol or of the .ud5 file. A capture from a real controller is needed first."
            }
            Self::TopWisdom => {
                "Needs hardware capture. There is no public description of the TopWisdom protocol or of the .out file. A capture from a real controller is needed first."
            }
            _ => return None,
        })
    }
    /// The wizard lets the user pick this type.
    pub fn selectable(self) -> bool {
        self.needs_capture().is_none()
    }
    /// The G-code firmware of `darkburn-gcodebots` this type runs (None for the built-in GRBL
    /// driver and for everything that is not G-code).
    pub fn lib_kind(self) -> Option<darkburn_gcodebots::profile::DeviceKind> {
        use darkburn_gcodebots::profile::DeviceKind as K;
        Some(match self {
            Self::Smoothie | Self::Emblaser2 | Self::SimulatorSmoothie => K::Smoothie,
            Self::Marlin | Self::SimulatorMarlin => K::Marlin,
            Self::Snapmaker | Self::SimulatorSnapmaker => K::Snapmaker,
            Self::LinuxCnc => K::LinuxCnc,
            Self::CustomGcode | Self::SimulatorCustom => K::Custom,
            _ => return None,
        })
    }
    /// A G-code device whose job is translated by `darkburn-gcodebots`.
    pub fn is_translated(self) -> bool {
        self.lib_kind().is_some()
    }
    /// Gweike machine concepts (files and the card simulator only).
    pub fn is_gweike(self) -> bool {
        self == Self::GweikeCardSimulator
    }
    pub fn flavor(self) -> Flavor {
        match self {
            Self::GrblHal => Flavor::GrblHal,
            Self::FluidNc => Flavor::FluidNc,
            Self::Smoothie | Self::Emblaser2 | Self::SimulatorSmoothie => Flavor::Smoothieware,
            Self::Marlin | Self::Snapmaker | Self::SimulatorMarlin | Self::SimulatorSnapmaker => Flavor::Marlin,
            Self::LinuxCnc => Flavor::LinuxCnc,
            _ => Flavor::Grbl,
        }
    }
    /// The connections the wizard offers for this type.
    pub fn connections(self) -> &'static [ConnectionKind] {
        match self {
            Self::Grbl | Self::GrblHal => &[ConnectionKind::Serial, ConnectionKind::Tcp],
            Self::FluidNc => &[ConnectionKind::Serial, ConnectionKind::Tcp],
            Self::Smoothie | Self::CustomGcode => &[ConnectionKind::Serial, ConnectionKind::Tcp],
            Self::Marlin | Self::Snapmaker | Self::Emblaser1 | Self::Emblaser2 => &[ConnectionKind::Serial],
            Self::Ruida => &[ConnectionKind::Udp, ConnectionKind::Usb],
            Self::Galvo => &[ConnectionKind::BoardUsb],
            _ => &[ConnectionKind::None],
        }
    }
    fn default_size(self) -> (f64, f64) {
        match self {
            Self::Ruida | Self::SimulatorRuida => (600.0, 400.0),
            Self::Snapmaker | Self::SimulatorSnapmaker => (320.0, 350.0),
            Self::Galvo | Self::SimulatorGalvo => (110.0, 110.0),
            Self::GweikeCardSimulator => (1371.0, 950.0),
            Self::Smoothie | Self::SimulatorSmoothie | Self::Marlin | Self::SimulatorMarlin => (300.0, 200.0),
            _ => (400.0, 400.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionKind {
    #[default]
    None,
    Serial,
    Tcp,
    Udp,
    Usb,
    /// A galvo board over USB (not built in yet).
    BoardUsb,
}

impl ConnectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None (built in)",
            Self::Serial => "USB serial",
            Self::Tcp => "TCP/IP",
            Self::Udp => "Ethernet (UDP)",
            Self::Usb => "USB (FTDI)",
            Self::BoardUsb => "USB (galvo board)",
        }
    }
}

/// How a serial port is chosen when connecting [LASER-18].
pub const AUTO_PORT: &str = "";

/// DSP-only settings (DS-16, DS-17).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DspSettings {
    pub u_axis: bool,
    pub dual_tube: bool,
    /// Add a number to the file name on every send.
    pub filename_increment: bool,
    /// Hide Start on machines that take uploads and run from the panel (LASER-23).
    pub disable_start: bool,
    pub check_bounds_when_framing: bool,
    pub network_timeout_ms: u32,
}

impl Default for DspSettings {
    fn default() -> Self {
        DspSettings {
            u_axis: false,
            dual_tube: false,
            filename_increment: true,
            disable_start: false,
            check_bounds_when_framing: true,
            network_timeout_ms: 3000,
        }
    }
}

/// What every device has (DS-01..DS-08, DS-19).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    /// DS-02: machine coordinates are negative from a zero at the far corner.
    pub negative_coords: bool,
    /// DS-03
    pub red_dot_x: f64,
    pub red_dot_y: f64,
    /// DS-04
    pub z_enabled: bool,
    pub z_reverse: bool,
    /// DS-05
    pub tab_pulse_width: f64,
    /// DS-06
    pub checklist_on: bool,
    pub checklist: Vec<String>,
    /// DS-07
    pub overwrite_files: bool,
    /// DS-08
    pub scan: crate::scan_offsets::ScanTuning,
    /// The most the Fire button may use, percent.
    pub fire_max_percent: f64,
    pub gcode: DeviceGcodeSettings,
    pub dsp: DspSettings,
    pub additional: AdditionalSettings,
    /// What the G-code firmwares of `darkburn-gcodebots` need beyond `gcode` [GCODE-01..05].
    pub gcode_ext: crate::gcode_machines::GcodeExt,
    /// The galvo board [DS-18].
    pub galvo: crate::galvo_settings::GalvoDeviceSettings,
    /// Gweike material and craft mapping [MCC-04].
    pub gweike: crate::gweike_workflow::GweikeSettings,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        DeviceSettings {
            negative_coords: false,
            red_dot_x: 0.0,
            red_dot_y: 0.0,
            z_enabled: false,
            z_reverse: false,
            tab_pulse_width: 0.1,
            checklist_on: false,
            checklist: vec![
                "Material is in place and flat".into(),
                "Air assist and extraction are on".into(),
                "Lid is closed and the area is clear".into(),
            ],
            overwrite_files: false,
            scan: crate::scan_offsets::ScanTuning::default(),
            fire_max_percent: 10.0,
            gcode: DeviceGcodeSettings::default(),
            dsp: DspSettings::default(),
            additional: AdditionalSettings::default(),
            gcode_ext: crate::gcode_machines::GcodeExt::default(),
            galvo: crate::galvo_settings::GalvoDeviceSettings::default(),
            gweike: crate::gweike_workflow::GweikeSettings::default(),
        }
    }
}

/// A saved copy of the controller's settings (GRBL `$$` text or Ruida JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub name: String,
    pub family: Family,
    pub text: String,
}

/// What a device remembers for itself (DEV-07).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceState {
    pub inches: bool,
    pub grid_spacing: f64,
    pub show_grid: bool,
    /// Job origin 9-point anchor, 0 to 8 reading order from the top left.
    pub job_origin: u8,
    /// Start from: 0 absolute, 1 current position, 2 user origin.
    pub start_from: u8,
    /// Dock layout of this device as the app's own JSON (`Prefs::panels`); `None` keeps the current one.
    pub panel_layout: Option<String>,
    /// Default layers (cut settings) as the app's own JSON; `None` keeps the project's.
    pub default_layers: Option<String>,
    /// Remembered serial port ("" = choose automatically).
    pub port: String,
    pub excluded_ports: Vec<String>,
    /// Material library files used with this device.
    pub libraries: Vec<String>,
    pub backups: Vec<BackupEntry>,
    pub macros: [String; 6],
}

impl Default for DeviceState {
    fn default() -> Self {
        DeviceState {
            inches: false,
            grid_spacing: 10.0,
            show_grid: true,
            job_origin: 6,
            start_from: 0,
            panel_layout: None,
            default_layers: None,
            port: AUTO_PORT.into(),
            excluded_ports: Vec::new(),
            libraries: Vec::new(),
            backups: Vec::new(),
            macros: Default::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub id: u64,
    pub name: String,
    pub kind: DeviceType,
    pub connection: ConnectionKind,
    /// TCP host, or the Ruida controller's IP.
    pub host: String,
    pub tcp_port: u16,
    /// Optional serial number of the FTDI adapter (empty = the first one).
    pub usb_serial: String,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Where the machine's 0,0 is.
    pub origin: Corner,
    pub auto_home: bool,
    pub settings: DeviceSettings,
    pub state: DeviceState,
}

impl Default for Device {
    fn default() -> Self {
        Device::new(DeviceType::Grbl)
    }
}

impl Device {
    pub fn new(kind: DeviceType) -> Self {
        let (w, h) = kind.default_size();
        let mut settings = DeviceSettings::default();
        settings.gcode.flavor = kind.flavor();
        if kind.is_simulator() {
            // Nothing can burn in a simulator, so its Fire button is on.
            settings.gcode.fire_button = true;
        }
        crate::gcode_machines::apply_preset(kind, &mut settings);
        if kind.family() == Family::Galvo {
            settings.galvo.field_size_mm = w;
        }
        Device {
            id: 0,
            name: match kind {
                DeviceType::SimulatorGrbl => "Simulator (GRBL), no hardware".into(),
                DeviceType::SimulatorRuida => "Simulator (Ruida), no hardware".into(),
                DeviceType::SimulatorSmoothie => "Simulator (Smoothieware), no hardware".into(),
                DeviceType::SimulatorMarlin => "Simulator (Marlin), no hardware".into(),
                DeviceType::SimulatorSnapmaker => "Simulator (Snapmaker), no hardware".into(),
                DeviceType::SimulatorCustom => "Simulator (custom G-code), no hardware".into(),
                DeviceType::SimulatorGalvo => "Simulator (galvo), no hardware".into(),
                DeviceType::GweikeCardSimulator => "Gweike card simulator, no hardware".into(),
                DeviceType::DesignOnly => "Design only".into(),
                k => format!("My {}", k.label()),
            },
            kind,
            connection: kind.connections()[0],
            host: match kind {
                DeviceType::Ruida => "192.168.1.100".into(),
                DeviceType::FluidNc | DeviceType::Grbl | DeviceType::GrblHal => String::new(),
                _ => String::new(),
            },
            tcp_port: 23,
            usb_serial: String::new(),
            width_mm: w,
            height_mm: h,
            origin: if matches!(kind, DeviceType::Ruida | DeviceType::SimulatorRuida) { Corner::TopLeft } else { Corner::BottomLeft },
            auto_home: false,
            settings,
            state: DeviceState::default(),
        }
    }

    pub fn family(&self) -> Family {
        self.kind.family()
    }

    /// What the job builder needs to know about this machine: bed, origin corner, negative
    /// coordinates, swapped axes and Z.
    pub fn job_device(&self) -> JobDevice {
        let (bed_w, bed_h) = self.bed();
        JobDevice {
            bed_w,
            bed_h,
            origin: self.origin,
            negative_coords: self.settings.negative_coords,
            swap_xy: self.settings.gcode.swap_xy,
            z_enabled: self.settings.z_enabled,
            z_reverse: self.settings.z_reverse,
            pointer_offset: [self.settings.red_dot_x, self.settings.red_dot_y],
            tab_pulse_width: self.settings.tab_pulse_width,
            ..JobDevice::default()
        }
    }
    pub fn is_simulator(&self) -> bool {
        self.kind.is_simulator()
    }
    pub fn is_design_only(&self) -> bool {
        self.kind == DeviceType::DesignOnly
    }
    /// The work area. A galvo's is the square marking field.
    pub fn bed(&self) -> (f64, f64) {
        if self.family() == Family::Galvo { crate::galvo_machine::bed_of(&self.settings.galvo) } else { (self.width_mm, self.height_mm) }
    }
    /// No laser can be connected: design only, the Gweike file workflow, or a type that still
    /// needs a hardware capture.
    pub fn has_no_laser(&self) -> bool {
        self.family().no_laser() || self.kind == DeviceType::LinuxCnc
    }

    /// Sanity-check what the wizard and the Device Settings dialog accept. The messages are shown
    /// next to the field.
    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.name.trim().is_empty() {
            v.push("Give the device a name.".into());
        }
        if !self.is_design_only() || self.width_mm > 0.0 {
            for (n, x) in [("Width", self.width_mm), ("Height", self.height_mm)] {
                if !(x.is_finite() && (1.0..=10_000.0).contains(&x)) {
                    v.push(format!("{n} must be between 1 and 10000 mm."));
                }
            }
        }
        match self.connection {
            ConnectionKind::Tcp if self.host.trim().is_empty() => v.push("Enter the host name or IP address.".into()),
            ConnectionKind::Udp if self.host.trim().parse::<std::net::IpAddr>().is_err() => {
                v.push("Enter the controller's IP address, like 192.168.1.100.".into())
            }
            ConnectionKind::Serial if self.settings.gcode.baud_rate == 0 => v.push("Pick a baud rate.".into()),
            _ => {}
        }
        if self.family() == Family::Galvo {
            v.extend(self.settings.galvo.validate());
        }
        if let Some(why) = self.kind.needs_capture() {
            v.push(why.to_owned());
        }
        if self.settings.gcode.s_max <= 0.0 && self.family() == Family::Grbl {
            v.push("S max must be above zero.".into());
        }
        v
    }

    /// The link for this device, or why there is none. `ports` is the current port list and is
    /// only used for the automatic serial choice.
    pub fn link(&self, ports: &[PortInfo]) -> Result<Link, String> {
        match (self.kind, self.connection) {
            (DeviceType::SimulatorGrbl, _) => Ok(Link::SimulatorGrbl),
            (DeviceType::SimulatorRuida, _) => Ok(Link::SimulatorRuida),
            (DeviceType::DesignOnly, _) => Err("This device is design only; there is no laser to connect.".into()),
            (DeviceType::SimulatorSmoothie, _) => Ok(Link::SimulatorGcode(SimGcode::Smoothie)),
            (DeviceType::SimulatorMarlin, _) => Ok(Link::SimulatorGcode(SimGcode::Marlin)),
            (DeviceType::SimulatorSnapmaker, _) => Ok(Link::SimulatorGcode(SimGcode::Snapmaker)),
            (DeviceType::SimulatorCustom, _) => Ok(Link::SimulatorGcode(SimGcode::Custom)),
            (DeviceType::SimulatorGalvo, _) => Ok(Link::SimulatorGalvo),
            (DeviceType::Galvo, _) => Ok(Link::GalvoUsb),
            (DeviceType::LinuxCnc, _) => Err("LinuxCNC is file output only. Use Save G-code and open the file in LinuxCNC.".into()),
            (DeviceType::GweikeCardSimulator, _) => {
                Err("The Gweike machine is a file workflow. Use the Gweike window to export files or run the card simulator.".into())
            }
            (k, _) if k.needs_capture().is_some() => Err(k.needs_capture().unwrap_or_default().to_owned()),
            (_, ConnectionKind::Serial) => {
                let port = pick_port(ports, &self.state.port, &self.state.excluded_ports)
                    .ok_or_else(|| "No serial port found. Plug the laser in and press Rescan.".to_string())?;
                Ok(Link::Serial { port, baud: self.settings.gcode.baud_rate })
            }
            (_, ConnectionKind::Tcp) => Ok(Link::Tcp { host: self.host.trim().to_owned(), port: self.tcp_port }),
            (_, ConnectionKind::Udp) => Ok(Link::Udp { ip: self.host.trim().to_owned() }),
            (_, ConnectionKind::Usb) => Ok(Link::Usb { serial: (!self.usb_serial.is_empty()).then(|| self.usb_serial.clone()) }),
            (_, ConnectionKind::BoardUsb) => Ok(Link::GalvoUsb),
            (_, ConnectionKind::None) => Err("Choose how this device connects.".into()),
        }
    }

    /// What the machine layer needs to connect.
    pub fn connect_spec(&self, ports: &[PortInfo]) -> Result<ConnectSpec, String> {
        let link = self.link(ports)?;
        let g = &self.settings.gcode;
        let config = ConnectConfig {
            toggle_dtr: g.dtr,
            dtr: true,
            synchronous: g.transfer == Transfer::Synchronous,
            s_max: (self.family() == Family::Grbl && !self.is_simulator()).then_some(g.s_max),
            bed: Some(self.bed()),
            max_fire_percent: self.settings.fire_max_percent.clamp(0.0, 100.0),
            overwrite_default: self.settings.overwrite_files.then_some(darkburn_ruida::OverwriteChoice::Overwrite),
            net_timeout_ms: (self.family() == Family::Ruida).then_some(self.settings.dsp.network_timeout_ms),
            gcode: self.kind.is_translated().then(|| std::sync::Arc::new(crate::gcode_machines::to_profile(self))),
            galvo: (self.family() == Family::Galvo).then(|| std::sync::Arc::new(self.settings.galvo.clone())),
            ..Default::default()
        };
        Ok(ConnectSpec { link, config })
    }
}

/// The serial port to use: the remembered one if it is still there and not excluded, else the
/// first USB-looking port that is not excluded [LASER-18].
pub fn pick_port(ports: &[PortInfo], remembered: &str, excluded: &[String]) -> Option<String> {
    let allowed: Vec<&PortInfo> = ports.iter().filter(|p| !excluded.iter().any(|e| e.eq_ignore_ascii_case(&p.name))).collect();
    if !remembered.is_empty()
        && let Some(p) = allowed.iter().find(|p| p.name.eq_ignore_ascii_case(remembered))
    {
        return Some(p.name.clone());
    }
    allowed.iter().find(|p| p.vid.is_some()).or_else(|| allowed.first()).map(|p| p.name.clone())
}

/// A laser Find My Laser thinks it can see. It is only a guess from the port list: nothing is
/// opened until the user presses Connect.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub port: String,
    pub label: String,
    pub kind: DeviceType,
    pub note: String,
}

/// Known USB-serial chips that GRBL boards and diode lasers use.
fn chip_name(vid: u16, pid: u16) -> Option<&'static str> {
    match (vid, pid) {
        (0x1A86, 0x7523) | (0x1A86, 0x55D4) => Some("CH340 / CH9102 USB serial (most diode lasers)"),
        (0x10C4, 0xEA60) => Some("CP210x USB serial"),
        (0x0403, 0x6001) | (0x0403, 0x6015) => Some("FTDI USB serial"),
        (0x2341, _) | (0x2A03, _) => Some("Arduino"),
        (0x303A, _) => Some("ESP32 (FluidNC)"),
        (0x0483, _) => Some("STM32 (grblHAL)"),
        (0x1D50, _) => Some("Smoothieboard USB"),
        _ => None,
    }
}

/// Find My Laser [DEV-02]: candidates from an already-enumerated port list. The caller gets the
/// list from `darkburn_grbl::list_ports()`, which only enumerates and never opens a port.
pub fn find_my_laser(ports: &[PortInfo]) -> Vec<Candidate> {
    ports
        .iter()
        .map(|p| {
            let chip = p.vid.zip(p.pid).and_then(|(v, i)| chip_name(v, i));
            let kind = match p.vid {
                Some(0x303A) => DeviceType::FluidNc,
                Some(0x0483) => DeviceType::GrblHal,
                Some(0x1D50) => DeviceType::Smoothie,
                _ => DeviceType::Grbl,
            };
            Candidate {
                port: p.name.clone(),
                label: p.label(),
                kind,
                note: match chip {
                    Some(c) => format!(
                        "{c}. Only the port list was read. Add it, press Connect, and DarkBurn tells you if the firmware that answers (GRBL, Smoothieware, Marlin, Snapmaker) is not the type you chose."
                    ),
                    None => "Unknown device. Add it as GRBL and press Connect; DarkBurn reads the firmware banner then and suggests the right type.".into(),
                },
            }
        })
        .collect()
}

/// A Ruida controller on the network cannot be found without sending packets, so DarkBurn asks
/// for its IP instead [DEV-02].
pub const RUIDA_DISCOVERY_NOTE: &str = "Ruida controllers on the network are not searched for (that would send packets). Choose Ruida in the wizard and type the controller's IP address.";

// ============================================================================================
// The store
// ============================================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Name,
    Type,
}

/// All devices, the selected one, and the order they are listed in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceStore {
    pub devices: Vec<Device>,
    pub active: Option<u64>,
    next_id: u64,
}

impl Default for DeviceStore {
    fn default() -> Self {
        let mut s = DeviceStore { devices: Vec::new(), active: None, next_id: 1 };
        let a = s.add(Device::new(DeviceType::SimulatorGrbl));
        s.add(Device::new(DeviceType::SimulatorRuida));
        s.add(Device::new(DeviceType::DesignOnly));
        s.active = Some(a);
        s
    }
}

impl DeviceStore {
    pub const FILE: &'static str = "devices.json";

    /// Add a device (its id is assigned here). Returns the id.
    pub fn add(&mut self, mut d: Device) -> u64 {
        d.id = self.next_id;
        self.next_id += 1;
        d.name = self.unique_name(&d.name, None);
        let id = d.id;
        self.devices.push(d);
        id
    }

    fn unique_name(&self, wanted: &str, except: Option<u64>) -> String {
        let base = wanted.trim();
        let base = if base.is_empty() { "Device" } else { base };
        let taken = |n: &str| self.devices.iter().any(|d| Some(d.id) != except && d.name.eq_ignore_ascii_case(n));
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_owned())
    }

    pub fn get(&self, id: u64) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Device> {
        self.devices.iter_mut().find(|d| d.id == id)
    }
    pub fn active(&self) -> Option<&Device> {
        self.active.and_then(|id| self.get(id))
    }
    pub fn active_mut(&mut self) -> Option<&mut Device> {
        let id = self.active?;
        self.get_mut(id)
    }
    pub fn select(&mut self, id: u64) -> bool {
        let ok = self.get(id).is_some();
        if ok {
            self.active = Some(id);
        }
        ok
    }

    /// Replace a device's contents (the Device Settings dialog's OK). The id stays.
    pub fn update(&mut self, mut d: Device) -> bool {
        let Some(pos) = self.devices.iter().position(|x| x.id == d.id) else { return false };
        d.name = self.unique_name(&d.name, Some(d.id));
        self.devices[pos] = d;
        true
    }

    /// DEV-04
    pub fn duplicate(&mut self, id: u64) -> Option<u64> {
        let mut d = self.get(id)?.clone();
        d.name = format!("{} copy", d.name);
        d.state.backups.clear();
        let pos = self.devices.iter().position(|x| x.id == id)? + 1;
        let new = self.add(d);
        let moved = self.devices.pop()?;
        self.devices.insert(pos, moved);
        Some(new)
    }

    /// DEV-04. The last device cannot be removed. If the active one goes, the first takes over.
    pub fn remove(&mut self, id: u64) -> bool {
        if self.devices.len() <= 1 || self.get(id).is_none() {
            return false;
        }
        self.devices.retain(|d| d.id != id);
        if self.active == Some(id) {
            self.active = self.devices.first().map(|d| d.id);
        }
        true
    }

    /// DEV-01: drag-reorder.
    pub fn move_device(&mut self, from: usize, to: usize) {
        if from >= self.devices.len() || to >= self.devices.len() || from == to {
            return;
        }
        let d = self.devices.remove(from);
        self.devices.insert(to, d);
    }

    /// DEV-01: sort by name or by type (then name).
    pub fn sort(&mut self, by: SortBy) {
        match by {
            SortBy::Name => self.devices.sort_by_key(|d| d.name.to_lowercase()),
            SortBy::Type => self.devices.sort_by_key(|d| (d.kind.label(), d.name.to_lowercase())),
        }
    }

    // ----- profiles (.dbdev) -----

    /// DEV-05: one device as a `.dbdev` file (JSON). Backups stay behind.
    pub fn export_profile(&self, id: u64) -> Option<String> {
        let mut d = self.get(id)?.clone();
        d.id = 0;
        d.state.backups.clear();
        serde_json::to_string_pretty(&Profile { format: PROFILE_FORMAT.into(), version: 1, device: d }).ok()
    }

    /// DEV-05: add a device from `.dbdev` text. Returns its id.
    pub fn import_profile(&mut self, text: &str) -> Result<u64, String> {
        // A device file written by `darkburn-gcodebots` (`{"darkburn_device": 1, ...}`) converts
        // into the one profile format here.
        if text.contains("\"darkburn_device\"") {
            let devices = crate::gcode_machines::import_library_profiles(text)?;
            let mut first = None;
            for d in devices {
                let errs = d.validate();
                if let Some(e) = errs.first() {
                    return Err(format!("The device file has a problem: {e}"));
                }
                let id = self.add(d);
                first.get_or_insert(id);
            }
            return first.ok_or_else(|| "The device file has no device in it.".to_string());
        }
        let p: Profile = serde_json::from_str(text).map_err(|e| format!("This is not a DarkBurn device file: {e}"))?;
        if p.format != PROFILE_FORMAT {
            return Err("This is not a DarkBurn device file.".into());
        }
        if p.version > 1 {
            return Err("This device file is from a newer DarkBurn.".into());
        }
        let mut d = p.device;
        d.state.backups.clear();
        let errs = d.validate();
        if !errs.is_empty() {
            return Err(format!("The device file has a problem: {}", errs[0]));
        }
        Ok(self.add(d))
    }

    // ----- saving -----

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// From stored JSON; unreadable text gives the defaults. Always has at least one device and a
    /// valid active one.
    pub fn from_json(text: &str) -> Self {
        let mut s: DeviceStore = serde_json::from_str(text).unwrap_or_default();
        s.sanitise();
        s
    }

    fn sanitise(&mut self) {
        if self.devices.is_empty() {
            *self = DeviceStore::default();
            return;
        }
        let max = self.devices.iter().map(|d| d.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.devices.len() {
            if self.devices[i].id == 0 || !seen.insert(self.devices[i].id) {
                self.devices[i].id = self.next_id;
                self.next_id += 1;
                seen.insert(self.devices[i].id);
            }
        }
        if self.active.is_none_or(|a| self.get(a).is_none()) {
            self.active = self.devices.first().map(|d| d.id);
        }
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(Self::FILE)).map(|t| Self::from_json(&t)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{}.tmp", Self::FILE));
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, dir.join(Self::FILE))
    }
}

pub const PROFILE_FORMAT: &str = "darkburn-device";

#[derive(Serialize, Deserialize)]
struct Profile {
    format: String,
    version: u32,
    device: Device,
}

// ============================================================================================
// The New Device wizard [DEV-03]
// ============================================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WizardStep {
    #[default]
    Type,
    Connection,
    NameAndSize,
    OriginAndHoming,
    Summary,
}

impl WizardStep {
    pub const ALL: [WizardStep; 5] = [Self::Type, Self::Connection, Self::NameAndSize, Self::OriginAndHoming, Self::Summary];
    pub fn label(self) -> &'static str {
        match self {
            Self::Type => "Type",
            Self::Connection => "Connection",
            Self::NameAndSize => "Name and size",
            Self::OriginAndHoming => "Origin and homing",
            Self::Summary => "Summary",
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Wizard {
    pub step: WizardStep,
    pub draft: Device,
}

impl Wizard {
    pub fn new() -> Self {
        Wizard { step: WizardStep::Type, draft: Device::new(DeviceType::Grbl) }
    }
    /// Start from a Find My Laser result.
    pub fn from_candidate(c: &Candidate) -> Self {
        let mut d = Device::new(c.kind);
        d.connection = ConnectionKind::Serial;
        d.state.port = c.port.clone();
        d.name = format!("Laser on {}", c.port);
        Wizard { step: WizardStep::NameAndSize, draft: d }
    }
    /// Pick the type; the rest of the draft follows the type's usual values.
    pub fn set_type(&mut self, kind: DeviceType) {
        let name = self.draft.name.clone();
        let keep_name = !name.starts_with("My ") && !name.starts_with("Simulator") && name != "Design only";
        self.draft = Device::new(kind);
        if keep_name {
            self.draft.name = name;
        }
    }
    /// Problems with the current step; empty means Next is allowed.
    pub fn problems(&self) -> Vec<String> {
        let d = &self.draft;
        match self.step {
            WizardStep::Type => Vec::new(),
            WizardStep::Connection => d.validate().into_iter().filter(|m| m.contains("host") || m.contains("IP") || m.contains("baud")).collect(),
            WizardStep::NameAndSize => {
                d.validate().into_iter().filter(|m| m.contains("name") || m.contains("Width") || m.contains("Height")).collect()
            }
            WizardStep::OriginAndHoming => Vec::new(),
            WizardStep::Summary => d.validate(),
        }
    }
    pub fn can_next(&self) -> bool {
        self.step != WizardStep::Summary && self.problems().is_empty()
    }
    /// Steps that do not apply to the chosen type are skipped (a simulator has no connection to
    /// set up; a design-only device has no origin or homing).
    fn applies(&self, step: WizardStep) -> bool {
        match step {
            WizardStep::Connection => !matches!(self.draft.kind.connections(), [ConnectionKind::None]),
            WizardStep::OriginAndHoming => !self.draft.is_design_only(),
            _ => true,
        }
    }
    pub fn next(&mut self) -> bool {
        if !self.can_next() {
            return false;
        }
        let mut i = self.step.index() + 1;
        while i < WizardStep::ALL.len() && !self.applies(WizardStep::ALL[i]) {
            i += 1;
        }
        self.step = WizardStep::ALL[i.min(WizardStep::ALL.len() - 1)];
        true
    }
    pub fn back(&mut self) {
        let mut i = self.step.index();
        while i > 0 {
            i -= 1;
            if self.applies(WizardStep::ALL[i]) {
                self.step = WizardStep::ALL[i];
                return;
            }
        }
    }
    /// The one-paragraph summary shown on the last page.
    pub fn summary(&self) -> Vec<(String, String)> {
        let d = &self.draft;
        let mut rows = vec![("Name".to_string(), d.name.clone()), ("Type".into(), d.kind.label().into())];
        if !matches!(d.kind.connections(), [ConnectionKind::None]) {
            rows.push((
                "Connection".into(),
                match d.connection {
                    ConnectionKind::Serial => {
                        format!("{} @ {} baud", if d.state.port.is_empty() { "automatic port" } else { &d.state.port }, d.settings.gcode.baud_rate)
                    }
                    ConnectionKind::Tcp => format!("{}:{}", d.host, d.tcp_port),
                    ConnectionKind::Udp => format!("UDP {}", d.host),
                    ConnectionKind::Usb => "FTDI USB".into(),
                    ConnectionKind::BoardUsb => "Galvo board over USB (simulator only for now)".into(),
                    ConnectionKind::None => "None".into(),
                },
            ));
        }
        if !d.is_design_only() {
            rows.push(("Work area".into(), format!("{} x {} mm", d.width_mm, d.height_mm)));
            rows.push(("Origin".into(), format!("{:?}{}", d.origin, if d.auto_home { ", homes on start" } else { "" })));
        }
        rows
    }
    /// Add the finished device to `store` and make it the active one.
    pub fn finish(&self, store: &mut DeviceStore) -> Result<u64, String> {
        let errs = self.draft.validate();
        if let Some(e) = errs.first() {
            return Err(e.clone());
        }
        let mut d = self.draft.clone();
        d.settings.gcode.auto_home = d.auto_home;
        if d.family() == Family::Galvo {
            // The size typed in the wizard is the lens field.
            d.settings.galvo.field_size_mm = d.width_mm;
        }
        let id = store.add(d);
        store.active = Some(id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, vid: Option<u16>, pid: Option<u16>) -> PortInfo {
        PortInfo { name: name.into(), vid, pid, ..Default::default() }
    }

    #[test]
    fn default_store_has_the_simulator_selected_and_labelled() {
        let s = DeviceStore::default();
        let a = s.active().unwrap();
        assert_eq!(a.kind, DeviceType::SimulatorGrbl);
        assert!(a.name.contains("Simulator") && a.name.contains("no hardware"));
        assert_eq!(a.link(&[]).unwrap(), Link::SimulatorGrbl);
        assert!(s.devices.iter().any(|d| d.is_design_only()));
        assert!(s.devices.iter().any(|d| d.kind == DeviceType::SimulatorRuida));
    }

    #[test]
    fn add_duplicate_remove_rename_keep_names_unique() {
        let mut s = DeviceStore::default();
        let id = s.add(Device::new(DeviceType::Grbl));
        let dup = s.duplicate(id).unwrap();
        let again = s.add(Device { name: "My GRBL".into(), ..Device::new(DeviceType::Grbl) });
        let names: Vec<_> = s.devices.iter().map(|d| d.name.to_lowercase()).collect();
        let mut u = names.clone();
        u.sort();
        u.dedup();
        assert_eq!(names.len(), u.len(), "names stay unique: {names:?}");
        assert_eq!(
            s.devices.iter().position(|d| d.id == dup),
            s.devices.iter().position(|d| d.id == id).map(|p| p + 1),
            "the copy sits after its original"
        );
        assert!(s.remove(again));
        s.select(id);
        assert!(s.remove(id));
        assert_ne!(s.active, Some(id));
        assert!(s.active().is_some());
        let mut tiny = DeviceStore { devices: vec![Device::new(DeviceType::Grbl)], active: None, next_id: 5 };
        tiny.devices[0].id = 4;
        assert!(!tiny.remove(4), "the last device stays");
    }

    #[test]
    fn reorder_and_sort() {
        let mut s = DeviceStore::default();
        let n = s.devices.len();
        let first = s.devices[0].id;
        s.move_device(0, n - 1);
        assert_eq!(s.devices[n - 1].id, first);
        s.move_device(0, 99);
        s.sort(SortBy::Name);
        let names: Vec<_> = s.devices.iter().map(|d| d.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        s.sort(SortBy::Type);
        let kinds: Vec<_> = s.devices.iter().map(|d| d.kind.label()).collect();
        let mut sk = kinds.clone();
        sk.sort();
        assert_eq!(kinds, sk);
    }

    #[test]
    fn profiles_round_trip_and_reject_junk() {
        let mut s = DeviceStore::default();
        let id = s.add(Device { name: "Atomstack".into(), width_mm: 410.0, ..Device::new(DeviceType::Grbl) });
        s.get_mut(id).unwrap().settings.gcode.start_script = "G54".into();
        s.get_mut(id).unwrap().state.backups.push(BackupEntry { name: "b".into(), family: Family::Grbl, text: "$30=1000".into() });
        let text = s.export_profile(id).unwrap();
        assert!(!text.contains("$30=1000"), "backups do not travel with a profile");
        let mut other = DeviceStore::default();
        let new = other.import_profile(&text).unwrap();
        let d = other.get(new).unwrap();
        assert_eq!((d.name.as_str(), d.width_mm, d.settings.gcode.start_script.as_str()), ("Atomstack", 410.0, "G54"));
        assert!(other.import_profile("not json").is_err());
        assert!(other.import_profile(r#"{"format":"other","version":1,"device":{}}"#).is_err());
        assert!(other.import_profile(r#"{"format":"darkburn-device","version":9,"device":{}}"#).is_err());
        let bad = text.replace("410.0", "-5.0");
        assert!(other.import_profile(&bad).is_err());
    }

    #[test]
    fn the_store_saves_and_loads() {
        let dir = std::env::temp_dir().join(format!("darkburn-dev-{}", std::process::id()));
        let mut s = DeviceStore::default();
        let id = s.add(Device { name: "Mine".into(), ..Device::new(DeviceType::Ruida) });
        s.get_mut(id).unwrap().state.inches = true;
        s.get_mut(id).unwrap().state.port = "COM7".into();
        s.select(id);
        s.save(&dir).unwrap();
        let back = DeviceStore::load(&dir);
        assert_eq!(back, s);
        assert_eq!(DeviceStore::from_json("garbage").devices.len(), 3);
        assert_eq!(DeviceStore::from_json(r#"{"devices":[]}"#).devices.len(), 3);
        let fixed = DeviceStore::from_json(r#"{"devices":[{"name":"a"},{"name":"b"}],"active":99}"#);
        assert_ne!(fixed.devices[0].id, fixed.devices[1].id);
        assert_eq!(fixed.active, Some(fixed.devices[0].id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_choice_remembers_and_excludes() {
        let ports = vec![port("COM1", None, None), port("COM3", Some(0x1A86), Some(0x7523)), port("COM4", Some(0x10C4), Some(0xEA60))];
        assert_eq!(pick_port(&ports, "", &[]), Some("COM3".into()), "auto prefers a USB serial chip");
        assert_eq!(pick_port(&ports, "COM4", &[]), Some("COM4".into()));
        assert_eq!(pick_port(&ports, "COM4", &["com4".into()]), Some("COM3".into()), "excluded ports are never picked");
        assert_eq!(pick_port(&ports, "COM9", &[]), Some("COM3".into()), "a remembered port that is gone falls back to auto");
        assert_eq!(pick_port(&ports, "", &["COM3".into(), "COM4".into()]), Some("COM1".into()));
        assert_eq!(pick_port(&[], "", &[]), None);
        let mut d = Device::new(DeviceType::Grbl);
        assert!(d.link(&[]).unwrap_err().contains("No serial port"));
        assert_eq!(d.link(&ports).unwrap(), Link::Serial { port: "COM3".into(), baud: 115_200 });
        d.connection = ConnectionKind::Tcp;
        d.host = "192.168.1.9".into();
        assert_eq!(d.link(&[]).unwrap(), Link::Tcp { host: "192.168.1.9".into(), port: 23 });
        assert!(Device::new(DeviceType::DesignOnly).link(&ports).is_err());
    }

    #[test]
    fn find_my_laser_only_describes_ports() {
        let ports = vec![port("COM3", Some(0x1A86), Some(0x7523)), port("COM8", Some(0x303A), Some(0x1001)), port("COM2", None, None)];
        let c = find_my_laser(&ports);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].kind, DeviceType::Grbl);
        assert!(c[0].note.contains("press Connect"));
        assert_eq!(c[1].kind, DeviceType::FluidNc);
        assert!(c[2].note.contains("Unknown"));
        assert!(find_my_laser(&[]).is_empty());
        assert!(RUIDA_DISCOVERY_NOTE.contains("not searched"));
    }

    #[test]
    fn wizard_walks_the_steps_and_skips_the_ones_that_do_not_apply() {
        let mut s = DeviceStore::default();
        let mut w = Wizard::new();
        assert_eq!(w.step, WizardStep::Type);
        w.set_type(DeviceType::Ruida);
        assert!(w.next());
        assert_eq!(w.step, WizardStep::Connection);
        w.draft.host = "nope".into();
        w.draft.connection = ConnectionKind::Udp;
        assert!(!w.next(), "a bad IP blocks Next");
        assert!(w.problems()[0].contains("IP"));
        w.draft.host = "192.168.1.50".into();
        assert!(w.next());
        assert_eq!(w.step, WizardStep::NameAndSize);
        w.draft.name = " ".into();
        assert!(!w.next());
        w.draft.name = "Big CO2".into();
        w.draft.width_mm = 900.0;
        assert!(w.next() && w.step == WizardStep::OriginAndHoming);
        w.draft.auto_home = true;
        assert!(w.next() && w.step == WizardStep::Summary);
        assert!(!w.can_next());
        assert!(w.summary().iter().any(|(k, v)| k == "Work area" && v.contains("900")));
        w.back();
        assert_eq!(w.step, WizardStep::OriginAndHoming);
        w.next();
        let id = w.finish(&mut s).unwrap();
        assert_eq!(s.active, Some(id));
        assert_eq!(s.get(id).unwrap().link(&[]).unwrap(), Link::Udp { ip: "192.168.1.50".into() });

        let mut sim = Wizard::new();
        sim.set_type(DeviceType::SimulatorGrbl);
        sim.next();
        assert_eq!(sim.step, WizardStep::NameAndSize, "a simulator has no connection page");
        let mut design = Wizard::new();
        design.set_type(DeviceType::DesignOnly);
        design.next();
        design.next();
        assert_eq!(design.step, WizardStep::Summary, "design only has no origin page");
        assert!(design.finish(&mut s).is_ok());
        let cand = Candidate { port: "COM5".into(), label: "COM5".into(), kind: DeviceType::Grbl, note: String::new() };
        let from = Wizard::from_candidate(&cand);
        assert_eq!((from.draft.state.port.as_str(), from.step), ("COM5", WizardStep::NameAndSize));
    }

    #[test]
    fn connect_spec_carries_the_device_settings() {
        let mut d = Device::new(DeviceType::Grbl);
        d.settings.gcode.transfer = Transfer::Synchronous;
        d.settings.gcode.dtr = true;
        d.settings.gcode.s_max = 255.0;
        d.settings.gcode.baud_rate = 57_600;
        d.settings.fire_max_percent = 3.0;
        let ports = vec![port("COM3", Some(0x1A86), Some(0x7523))];
        let spec = d.connect_spec(&ports).unwrap();
        assert_eq!(spec.link, Link::Serial { port: "COM3".into(), baud: 57_600 });
        assert!(spec.config.synchronous && spec.config.toggle_dtr);
        assert_eq!((spec.config.s_max, spec.config.max_fire_percent, spec.config.bed), (Some(255.0), 3.0, Some((400.0, 400.0))));
        let sim = Device::new(DeviceType::SimulatorGrbl).connect_spec(&[]).unwrap();
        assert!(sim.link.is_simulator() && sim.config.s_max.is_none());
        let mut r = Device::new(DeviceType::Ruida);
        r.settings.overwrite_files = true;
        assert_eq!(r.connect_spec(&[]).unwrap().config.overwrite_default, Some(darkburn_ruida::OverwriteChoice::Overwrite));
    }

    #[test]
    fn validate_catches_bad_values() {
        let mut d = Device::new(DeviceType::Grbl);
        assert!(d.validate().is_empty());
        d.width_mm = f64::NAN;
        d.name = String::new();
        d.settings.gcode.s_max = 0.0;
        assert_eq!(d.validate().len(), 3);
    }

    #[test]
    fn additional_settings_never_give_the_estimator_zeroes() {
        let a = AdditionalSettings { accel: 0.0, rapid_speed: -1.0, ..Default::default() };
        let m = a.motion();
        assert!(m.accel >= 1.0 && m.rapid_speed >= 1.0);
    }
}
