//! The ways DarkBurn reaches a laser beyond a plain serial port or socket, and the pieces around
//! them [CONN-01..CONN-09, GRBL-01..GRBL-05, GRBL-11, GRBL-12, DEV-02, DEV-06, LASER-20].
//!
//! * [`SimFirmware`]: which GRBL-family firmware the built-in simulator pretends to be, so every
//!   flavour can be tried with no hardware.
//! * [`ConnSettings`]: what a device stores for HTTP, raw USB, a network bridge and DTR.
//! * [`HttpTransport`] (G-code as HTTP requests) and [`BridgeTransport`] (Ruida through a TCP
//!   relay). Both sit behind the driver crates' transport traits.
//! * The automation socket: a local UDP listener for `START` and `STATUS`, off by default.
//! * The network bridge wizard, the bridge rescan and the "this looks like an Ortur" message.
//!
//! Hardware safety: nothing here opens a link by itself. The transports are built only by
//! `machine_api::open_real`, after the user's Connect click. In test builds only a loopback
//! address (a listener the test started itself) is accepted. The bridge probe follows the same
//! rule, and an agent can never run it.

use std::collections::VecDeque;
use std::io::{self, Read as _, Write as _};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs as _, UdpSocket};
use std::time::Duration;

use darkburn_grbl as grbl;
use darkburn_ruida as ruida;
use darkburn_ui::ui_kit::{self, Action, BackingTooltipExt as _, Button, StatusTone};
use egui::{Context, Id, RichText, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::commands::Cmd;
use crate::devices::{ConnectionKind, Device, DeviceStore, DeviceType, Wizard, WizardStep};
use crate::machine_api::{ConnectConfig, DriverAdapter, GrblAdapter, LaserState, Link, MachineError, RuidaAdapter};
use crate::machine_ui::Source;
use crate::shell::DarkBurn;

// ============================================================================================
// The simulator's firmware
// ============================================================================================

/// What the in-memory GRBL controller pretends to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimFirmware {
    #[default]
    Grbl11h,
    /// GRBL 1.1e: constant power only [GRBL-02].
    Grbl11e,
    /// GRBL 0.9j: the older status text, no `$J` jogging, no overrides [GRBL-02].
    Grbl09,
    /// grbl-LPC on a Smoothieboard [GRBL-03].
    GrblLpc,
    /// grblHAL [GRBL-04].
    GrblHal,
    /// FluidNC [GRBL-05].
    FluidNc,
    // Diode lasers that name themselves [GRBL-12].
    Ortur,
    XTool,
    Atomstack,
    Sculpfun,
}

impl SimFirmware {
    pub const ALL: [SimFirmware; 10] = [
        Self::Grbl11h,
        Self::Grbl11e,
        Self::Grbl09,
        Self::GrblLpc,
        Self::GrblHal,
        Self::FluidNc,
        Self::Ortur,
        Self::XTool,
        Self::Atomstack,
        Self::Sculpfun,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Grbl11h => "GRBL 1.1h",
            Self::Grbl11e => "GRBL 1.1e (M3 only)",
            Self::Grbl09 => "GRBL 0.9 (M3 only)",
            Self::GrblLpc => "GRBL-LPC",
            Self::GrblHal => "grblHAL",
            Self::FluidNc => "FluidNC",
            Self::Ortur => "Ortur Laser Master 2 Pro",
            Self::XTool => "xTool D1 Pro",
            Self::Atomstack => "Atomstack",
            Self::Sculpfun => "Sculpfun",
        }
    }
    /// No dynamic power: jobs are written with M3.
    pub fn m3_only(self) -> bool {
        matches!(self, Self::Grbl11e | Self::Grbl09)
    }
    /// The emulator set-up. The vendor lines are the ones owners have posted publicly (see
    /// `darkburn_grbl::detect`); the Atomstack and Sculpfun names are the form ESP32 boards
    /// print and still need a look at a real machine.
    pub fn emulator_config(self) -> grbl::EmulatorConfig {
        use grbl::EmulatorFlavor as F;
        let plain = |f| grbl::EmulatorConfig::flavor(f);
        match self {
            Self::Grbl11h => plain(F::Grbl11h),
            Self::Grbl11e => plain(F::Grbl11e),
            Self::Grbl09 => plain(F::Grbl09),
            Self::GrblLpc => plain(F::GrblLpc),
            Self::GrblHal => plain(F::GrblHal),
            Self::FluidNc => plain(F::FluidNc),
            Self::Ortur => plain(F::Grbl11h).with_vendor_text(
                &["Ortur Laser Master 2 Pro Ready!"],
                &["[ORIGIN: China]", "[PRODUCER: ORTUR]", "[AUTHOR: ORTUR]", "[MODEL: Ortur Laser Master 2 Pro S1]", "[OLF: 172]"],
            ),
            Self::XTool => plain(F::Grbl11h).with_vendor_text(&[], &["[xTool D1Pro:ver 40.31.002.01 B5]"]),
            Self::Atomstack => plain(F::Grbl11h).with_vendor_text(&["[MSG:Using machine:ATOMSTACK_A20]"], &[]),
            Self::Sculpfun => plain(F::Grbl11h).with_vendor_text(&["[MSG:Using machine:SCULPFUN_S30]"], &[]),
        }
    }
}

/// The GRBL simulator of `machine_api`: the adapter and a handle to the in-memory board.
pub fn grbl_simulator(cfg: &ConnectConfig) -> (GrblAdapter, grbl::EmulatorHandle) {
    let mut e = cfg.sim_firmware.emulator_config().with_time_scale(cfg.sim_time_scale.max(0.1));
    e.record_lines = true;
    if cfg.sim_homing {
        e = e.with_setting(22, 1.0);
    }
    if let Some((w, h)) = cfg.bed {
        e = e.with_setting(130, w).with_setting(131, h);
    }
    let emu = grbl::EmulatorHandle::new(grbl::GrblEmulator::new(e));
    (GrblAdapter::new(Box::new(emu.transport()), cfg, true, Some(emu.clone())), emu)
}

/// Whether a firmware name such as `GRBL 1.1e` is a GRBL without dynamic power (before 1.1f).
pub fn grbl_is_m3_only(firmware: &str) -> bool {
    let Some(v) = firmware.strip_prefix("GRBL ") else { return false };
    grbl::protocol::parse_version(v).is_some_and(|(major, minor, letter)| (major, minor, letter.unwrap_or('a')) < (1, 1, 'f'))
}

// ============================================================================================
// What a device stores
// ============================================================================================

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnSettings {
    /// Simulator (GRBL) only.
    pub sim_firmware: SimFirmware,
    /// HTTP [CONN-07]: the port and the start of the request; the line is added URL-encoded.
    pub http_port: u16,
    pub http_path: String,
    /// Raw USB [CONN-05]: vendor and product ID as hex text, the interface, and the two bulk
    /// endpoint numbers (1 to 15).
    pub usb_vid: String,
    pub usb_pid: String,
    pub usb_interface: u8,
    pub usb_ep_out: u8,
    pub usb_ep_in: u8,
    /// Network bridge [CONN-03].
    pub bridge_port: u16,
    /// Keep the DTR line on while connected [GRBL-11]. Some boards stay in reset while it is on.
    pub dtr_enabled: bool,
}

impl Default for ConnSettings {
    fn default() -> Self {
        ConnSettings {
            sim_firmware: SimFirmware::default(),
            http_port: 80,
            http_path: "/command?plain=".into(),
            usb_vid: String::new(),
            usb_pid: String::new(),
            usb_interface: 0,
            usb_ep_out: 2,
            usb_ep_in: 2,
            bridge_port: BRIDGE_PORT,
            dtr_enabled: true,
        }
    }
}

fn hex16(s: &str) -> u16 {
    u16::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).unwrap_or(0)
}

impl ConnSettings {
    pub fn usb_address(&self, serial: &str) -> grbl::RawUsbAddress {
        grbl::RawUsbAddress {
            vid: hex16(&self.usb_vid),
            pid: hex16(&self.usb_pid),
            serial: (!serial.trim().is_empty()).then(|| serial.trim().to_owned()),
            interface: self.usb_interface,
            ep_out: self.usb_ep_out & 0x0F,
            ep_in: 0x80 | (self.usb_ep_in & 0x0F),
        }
    }
}

// ============================================================================================
// Loopback rule
// ============================================================================================

/// `host` is this computer, written as an IP address (a name is never trusted).
pub fn is_loopback_host(host: &str) -> bool {
    host.trim().parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The only real links a test build may open: network links to this computer, where the test
/// itself runs the listener.
pub fn loopback_only(link: &Link) -> bool {
    match link {
        Link::Tcp { host, .. } | Link::Http { host, .. } | Link::Bridge { host, .. } => is_loopback_host(host),
        _ => false,
    }
}

fn resolve(host: &str, port: u16) -> io::Result<SocketAddr> {
    (host.trim(), port).to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}")))
}

// ============================================================================================
// HTTP [CONN-07]
// ============================================================================================

fn url_encode(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// G-code over HTTP: every line and every real-time character is one `GET` request whose
/// address is `http://host:port` + the device's request path + the URL-encoded text. Whatever
/// the controller puts in the reply body is read back as its answer.
///
/// This is the request form of FluidNC's and ESP3D's web interface (`/command?plain=`). What a
/// given controller answers there differs between firmwares and versions, so a real machine has
/// to confirm it (docs/HARDWARE-CHECKS.md).
pub struct HttpTransport {
    agent: ureq::Agent,
    base: String,
    line: Vec<u8>,
    pending: VecDeque<u8>,
}

impl HttpTransport {
    /// Opens nothing: HTTP has no standing connection. The first request is the first contact.
    pub fn new(host: &str, port: u16, path: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(5)))
            .timeout_connect(Some(Duration::from_secs(3)))
            .max_redirects(0)
            .build()
            .into();
        let path = if path.starts_with('/') { path.to_owned() } else { format!("/{path}") };
        HttpTransport { agent, base: format!("http://{}:{port}{path}", host.trim()), line: Vec::new(), pending: VecDeque::new() }
    }

    fn request(&mut self, text: &[u8]) -> io::Result<()> {
        let url = format!("{}{}", self.base, url_encode(text));
        let mut resp = self.agent.get(&url).call().map_err(io::Error::other)?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().map_err(io::Error::other)?;
        if !(200..300).contains(&status) {
            return Err(io::Error::other(format!("HTTP {status}")));
        }
        self.pending.extend(body.bytes());
        if !body.is_empty() && !body.ends_with('\n') {
            self.pending.push_back(b'\n');
        }
        Ok(())
    }
}

impl grbl::Transport for HttpTransport {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        for &b in data {
            if grbl::protocol::rt::is_realtime(b) {
                self.request(&[b])?;
            } else if b == b'\n' {
                let line = std::mem::take(&mut self.line);
                let line = line.trim_ascii();
                if !line.is_empty() {
                    self.request(line)?;
                }
            } else {
                self.line.push(b);
            }
        }
        Ok(())
    }
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        if self.pending.is_empty() {
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
            return Ok(0);
        }
        let n = buf.len().min(self.pending.len());
        for (slot, b) in buf.iter_mut().zip(self.pending.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }
    fn name(&self) -> String {
        self.base.clone()
    }
}

/// Reached only through `machine_api::open_real`.
pub fn open_http(host: &str, port: u16, path: &str, config: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    if host.trim().is_empty() {
        return Err(MachineError::Invalid("Enter the host name or IP address.".into()));
    }
    // One request per line: the next line waits for the answer to the one before.
    let mut cfg = config.clone();
    cfg.synchronous = true;
    Ok(crate::gcode_machines::adapter_for(Box::new(HttpTransport::new(host, port, path)), &cfg))
}

// ============================================================================================
// Raw USB [CONN-05]
// ============================================================================================

/// Reached only through `machine_api::open_real`.
pub fn open_raw_usb(addr: &grbl::RawUsbAddress, config: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    if let Some(p) = addr.problem() {
        return Err(MachineError::Invalid(p));
    }
    #[cfg(feature = "rawusb")]
    {
        let pipe = grbl::UsbPipe::open(addr).map_err(|e| MachineError::Driver(format!("Could not open {}: {e}", addr.label())))?;
        Ok(crate::gcode_machines::adapter_for(Box::new(grbl::RawUsbTransport::new(pipe)), config))
    }
    #[cfg(not(feature = "rawusb"))]
    {
        let _ = config;
        Err(MachineError::Unsupported("this build has no raw USB support".into()))
    }
}

// ============================================================================================
// Network bridge [CONN-03, LASER-20]
// ============================================================================================

/// The TCP port a network bridge listens on.
pub const BRIDGE_PORT: u16 = 5005;
/// Every relayed packet starts with this byte, then its length as two bytes, high byte first.
pub const BRIDGE_MARK: u8 = 0x4C;

/// One relayed packet as it goes over TCP.
pub fn bridge_frame(packet: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(packet.len() + 3);
    f.push(BRIDGE_MARK);
    f.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    f.extend_from_slice(packet);
    f
}

/// Takes one whole packet off the front of `buf`, if one is there. Bytes before a mark are
/// dropped.
pub fn bridge_unframe(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = buf.iter().position(|b| *b == BRIDGE_MARK)?;
    buf.drain(..start);
    if buf.len() < 3 {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([buf[1], buf[2]]));
    if buf.len() < 3 + len {
        return None;
    }
    let packet = buf[3..3 + len].to_vec();
    buf.drain(..3 + len);
    Some(packet)
}

/// A Ruida controller behind a network bridge: the bridge takes TCP from DarkBurn and passes
/// each packet to the controller's UDP port, and the answers back.
///
/// The packets are the same datagrams the UDP link sends (checksum included), each wrapped as
/// [`bridge_frame`]. That framing comes from one public description of such a bridge and is not
/// confirmed against a real one (docs/HARDWARE-CHECKS.md). The jog port is not relayed.
pub struct BridgeTransport {
    stream: TcpStream,
    rx: Vec<u8>,
    label: String,
}

impl BridgeTransport {
    /// Opens the TCP connection. Called only from [`open_bridge`].
    pub fn connect(host: &str, port: u16, timeout: Duration) -> io::Result<Self> {
        let addr = resolve(host, port)?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        Ok(BridgeTransport { stream, rx: Vec::new(), label: format!("bridge {addr}") })
    }
}

impl ruida::Transport for BridgeTransport {
    fn framing(&self) -> ruida::transport::Framing {
        ruida::transport::Framing::Udp
    }
    fn send(&mut self, channel: ruida::transport::Channel, data: &[u8]) -> ruida::Result<()> {
        if channel == ruida::transport::Channel::Jog {
            return Err(ruida::RuidaError::Unsupported("the network bridge relays the data port only".into()));
        }
        self.stream.write_all(&bridge_frame(data))?;
        Ok(())
    }
    fn recv(&mut self, channel: ruida::transport::Channel, timeout: Duration) -> ruida::Result<Option<Vec<u8>>> {
        if channel == ruida::transport::Channel::Jog {
            return Err(ruida::RuidaError::Unsupported("the network bridge relays the data port only".into()));
        }
        if let Some(p) = bridge_unframe(&mut self.rx) {
            return Ok(Some(p));
        }
        self.stream.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        let mut buf = [0u8; 2048];
        match self.stream.read(&mut buf) {
            Ok(0) => Err(ruida::RuidaError::Transport("the bridge closed the connection".into())),
            Ok(n) => {
                self.rx.extend_from_slice(&buf[..n]);
                Ok(bridge_unframe(&mut self.rx))
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn describe(&self) -> String {
        self.label.clone()
    }
}

/// Reached only through `machine_api::open_real`.
pub fn open_bridge(host: &str, port: u16, config: &ConnectConfig) -> Result<Box<dyn DriverAdapter>, MachineError> {
    let t = BridgeTransport::connect(host, port, Duration::from_secs(5))
        .map_err(|e| MachineError::Driver(format!("Could not reach the bridge at {host}:{port}: {e}")))?;
    Ok(Box::new(RuidaAdapter::new(Box::new(t), config, false, None)))
}

/// Rescan [LASER-20]: see whether a bridge answers at `host:port`. It opens a TCP connection
/// and closes it again; nothing is sent. Only the window may ask (an agent never), and a test
/// build only looks at this computer.
pub fn probe_bridge(host: &str, port: u16, src: Source) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("Enter the bridge's host name or IP address first.".into());
    }
    if src == Source::Api && !cfg!(test) {
        return Err("Looking for a bridge opens a network connection. Use the Rescan button in the window.".into());
    }
    if cfg!(test) && !is_loopback_host(host) {
        return Err("Network connections are disabled in test builds.".into());
    }
    let addr = resolve(host, port).map_err(|e| format!("Could not find {host}: {e}"))?;
    match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(_) => Ok(format!("A bridge answered at {host}:{port}.")),
        Err(e) => Err(format!("Nothing answered at {host}:{port} ({e}). Check that the bridge is on and on the same network.")),
    }
}

/// The network bridge setup wizard [DEV-06].
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeWizard {
    pub step: usize,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub width: f64,
    pub height: f64,
    /// What the last test said, and whether it was good news.
    pub tested: Option<(String, bool)>,
}

impl Default for BridgeWizard {
    fn default() -> Self {
        BridgeWizard {
            step: 0,
            host: String::new(),
            port: BRIDGE_PORT,
            name: "Laser on the bridge".into(),
            width: 600.0,
            height: 400.0,
            tested: None,
        }
    }
}

impl BridgeWizard {
    pub const STEPS: [&'static str; 3] = ["Bridge", "Laser", "Summary"];

    /// What stops the current step; empty means Next (or Add) is allowed.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.host.trim().is_empty() {
            v.push("Enter the bridge's host name or IP address.".to_owned());
        }
        if self.step >= 1 {
            if self.name.trim().is_empty() {
                v.push("Give the laser a name.".to_owned());
            }
            for (n, x) in [("Width", self.width), ("Height", self.height)] {
                if !(x.is_finite() && (1.0..=10_000.0).contains(&x)) {
                    v.push(format!("{n} must be between 1 and 10000 mm."));
                }
            }
        }
        v
    }
    pub fn device(&self) -> Device {
        let mut d = Device::new(DeviceType::Ruida);
        d.connection = ConnectionKind::Bridge;
        d.host = self.host.trim().to_owned();
        d.settings.conn.bridge_port = self.port;
        d.name = self.name.trim().to_owned();
        d.width_mm = self.width;
        d.height_mm = self.height;
        d
    }
    /// Add the device and select it.
    pub fn finish(&self, store: &mut DeviceStore) -> Result<u64, String> {
        let d = self.device();
        if let Some(e) = self.problems().into_iter().chain(d.validate()).next() {
            return Err(e);
        }
        let id = store.add(d);
        store.active = Some(id);
        Ok(id)
    }
}

// ============================================================================================
// Automation socket [CONN-09]
// ============================================================================================

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationSettings {
    pub enabled: bool,
    /// UDP port DarkBurn listens on.
    pub port: u16,
    /// UDP port the answer goes to on the sender's address; 0 answers the port it came from.
    pub reply_port: u16,
    /// Listen on every network interface instead of this computer only.
    pub allow_remote: bool,
}

impl Default for AutomationSettings {
    fn default() -> Self {
        AutomationSettings { enabled: false, port: 19840, reply_port: 19841, allow_remote: false }
    }
}

/// The open socket and what came in.
#[derive(Default)]
pub struct AutomationRuntime {
    socket: Option<UdpSocket>,
    applied: Option<AutomationSettings>,
    /// The port really bound (differs from the setting when that is 0).
    pub local_port: Option<u16>,
    pub error: Option<String>,
    /// The last commands and their answers, newest last.
    pub log: VecDeque<String>,
}

impl AutomationRuntime {
    fn apply(&mut self, want: &AutomationSettings) {
        if self.applied.as_ref() == Some(want) {
            return;
        }
        self.applied = Some(want.clone());
        self.socket = None;
        self.local_port = None;
        self.error = None;
        if !want.enabled {
            return;
        }
        // Tests stay on this computer whatever the setting says.
        let ip: IpAddr = if want.allow_remote && !cfg!(test) { std::net::Ipv4Addr::UNSPECIFIED.into() } else { std::net::Ipv4Addr::LOCALHOST.into() };
        match UdpSocket::bind((ip, want.port)).and_then(|s| s.set_nonblocking(true).map(|_| s)) {
            Ok(s) => {
                self.local_port = s.local_addr().ok().map(|a| a.port());
                self.socket = Some(s);
            }
            Err(e) => self.error = Some(format!("Could not listen on UDP port {}: {e}", want.port)),
        }
    }
}

/// The one-word state the `STATUS` command answers with.
pub fn automation_status(app: &DarkBurn) -> String {
    let s = app.machine.handle.snapshot();
    match s.state {
        LaserState::Busy => match s.job.as_ref().and_then(|j| j.fraction) {
            Some(f) => format!("BUSY {:.0}%", (f * 100.0).clamp(0.0, 100.0)),
            None => "BUSY".into(),
        },
        other => other.label().to_ascii_uppercase(),
    }
}

/// Answer one automation command. `START`, `STOP`, `PAUSE` and `RESUME` run the same commands
/// as the buttons in the Laser panel, with the same checks.
pub fn automation_command(app: &mut DarkBurn, text: &str) -> String {
    let cmd = text.trim().to_ascii_uppercase();
    let run = |app: &mut DarkBurn, id: &str| match crate::machine_ui::dispatch(app, id, &json!({}), Source::Ui) {
        Some(Ok(_)) => "OK".to_owned(),
        Some(Err(e)) => format!("ERROR: {e}"),
        None => "ERROR: not available".to_owned(),
    };
    match cmd.as_str() {
        "PING" => "OK".into(),
        "STATUS" => automation_status(app),
        "START" => {
            if !app.machine.handle.is_connected() {
                return "ERROR: no laser is connected".into();
            }
            // Built at once, then the same path as the Start button (checklist, bounds).
            match crate::machine_job::start_now(app, crate::machine_job::Action::Start { from_op: None }, false) {
                Ok(v) if v.get("waiting_for_checklist").and_then(Value::as_bool) == Some(true) => "WAITING: tick the checklist in DarkBurn".into(),
                Ok(_) => "OK".into(),
                Err(e) => format!("ERROR: {e}"),
            }
        }
        "STOP" => run(app, "machine.stop"),
        "PAUSE" => run(app, "machine.pause"),
        "RESUME" => run(app, "machine.resume"),
        _ => "ERROR: unknown command (use PING, STATUS, START, PAUSE, RESUME or STOP)".into(),
    }
}

/// Once per frame: open or close the socket as the setting says, and answer what came in.
pub fn automation_tick(app: &mut DarkBurn) {
    let want = app.machine.devices.automation.clone();
    app.machine.conn.automation.apply(&want);
    for _ in 0..16 {
        let mut buf = [0u8; 1024];
        let Some(socket) = app.machine.conn.automation.socket.as_ref() else { return };
        let Ok((n, from)) = socket.recv_from(&mut buf) else { return };
        let text = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
        let answer = automation_command(app, &text);
        let to = SocketAddr::new(from.ip(), if want.reply_port == 0 { from.port() } else { want.reply_port });
        let rt = &mut app.machine.conn.automation;
        if let Some(s) = rt.socket.as_ref() {
            let _ = s.send_to(answer.as_bytes(), to);
        }
        if rt.log.len() >= 50 {
            rt.log.pop_front();
        }
        rt.log.push_back(format!("{text} -> {answer}"));
    }
}

// ============================================================================================
// UI state
// ============================================================================================

#[derive(Default)]
pub struct ConnUi {
    pub automation: AutomationRuntime,
    pub automation_open: bool,
    pub bridge: Option<BridgeWizard>,
    /// What the last bridge rescan in the Laser panel said, and whether it was good news.
    pub bridge_status: Option<(String, bool)>,
    /// USB devices listed for Find My Laser (listing opens nothing).
    pub usb_devices: Vec<grbl::UsbDeviceInfo>,
    last_detected: Option<grbl::DetectedLaser>,
}

/// Once per frame, with or without a window.
pub fn frame_tick(app: &mut DarkBurn) {
    automation_tick(app);
    // Told after the "Connected" message, not before it.
    let connected = app.machine.handle.is_connected();
    let now = app.machine.handle.snapshot().detected.clone().filter(|_| connected);
    if now != app.machine.conn.last_detected {
        if let Some(d) = &now {
            app.machine.say(format!("This looks like {}. Use Detected laser in the Laser panel to take its name and size.", d.label()));
        }
        app.machine.conn.last_detected = now;
    }
}

// ============================================================================================
// Find My Laser: USB devices [DEV-02]
// ============================================================================================

/// List the USB devices again. Reads what the operating system already knows; opens nothing.
pub fn rescan_usb(app: &mut DarkBurn) {
    app.machine.conn.usb_devices = grbl::list_usb_devices();
}

/// A New Device wizard started from a USB device in the list.
pub fn wizard_from_usb(info: &grbl::UsbDeviceInfo) -> Wizard {
    let mut d = Device::new(DeviceType::Grbl);
    d.connection = ConnectionKind::RawUsb;
    d.settings.conn.usb_vid = format!("{:04X}", info.vid);
    d.settings.conn.usb_pid = format!("{:04X}", info.pid);
    d.usb_serial = info.serial_number.clone().unwrap_or_default();
    d.name = format!("Laser on {}", info.label());
    Wizard { step: WizardStep::Connection, draft: d }
}

// ============================================================================================
// Detected laser [GRBL-12]
// ============================================================================================

/// What the connected controller says about itself: the recognised laser, and the size and
/// S value read from its own settings.
pub fn detected_json(app: &DarkBurn) -> Value {
    let s = app.machine.handle.snapshot();
    let setting = |n: u16| s.grbl_settings.get(&n).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    json!({
        "detected": s.detected.as_ref().map(|d| json!({ "brand": d.brand, "model": d.model, "firmware": d.firmware, "label": d.label() })),
        "firmware": s.firmware,
        "width_mm": setting(130),
        "height_mm": setting(131),
        "s_max": setting(30),
    })
}

/// Take the detected laser's name, and the work area and S value the controller reports, into
/// the selected device.
pub fn adopt_detected(app: &mut DarkBurn) -> Result<Value, String> {
    let info = detected_json(app);
    if !app.machine.handle.is_connected() {
        return Err("Connect to the laser first.".into());
    }
    let Some(active) = app.machine.devices.active else { return Err("No device is selected.".into()) };
    let label = info["detected"]["label"].as_str().map(str::to_owned);
    let (w, h, s_max) = (info["width_mm"].as_f64(), info["height_mm"].as_f64(), info["s_max"].as_f64());
    if label.is_none() && w.is_none() && h.is_none() {
        return Err("The controller did not say what it is, and its size settings were not read.".into());
    }
    let Some(dev) = app.machine.devices.get_mut(active) else { return Err("No device is selected.".into()) };
    if let Some(l) = &label
        && !dev.is_simulator()
    {
        dev.name = l.clone();
    }
    if let (Some(w), Some(h)) = (w, h) {
        dev.width_mm = w;
        dev.height_mm = h;
    }
    if let Some(s) = s_max {
        dev.settings.gcode.s_max = s;
    }
    let name = dev.name.clone();
    app.machine.applied_reset();
    crate::machine_ui::adopt_device_state(app);
    Ok(json!({ "ok": true, "name": name, "width_mm": w, "height_mm": h, "s_max": s_max }))
}

// ============================================================================================
// Commands
// ============================================================================================

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    a(
        "device.set_connection",
        "Set How a Device Connects",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"connection":{"enum":["Serial","Bluetooth","Tcp","Http","RawUsb","Udp","Usb","Bridge"]},"host":{"type":"string"},"port":{"type":"integer"},"baud":{"type":"integer"},"serial_port":{"type":"string"},"http_path":{"type":"string"},"usb_vid":{"type":"string"},"usb_pid":{"type":"string"},"usb_interface":{"type":"integer"},"usb_ep_out":{"type":"integer"},"usb_ep_in":{"type":"integer"},"pulse_dtr":{"type":"boolean"},"dtr_enabled":{"type":"boolean"},"sim_firmware":{"type":"string"}}}"#,
    ),
    a("device.find", "Find My Laser: List Ports and USB Devices", ""),
    a("device.detected", "What the Connected Laser Says It Is", ""),
    a("device.adopt_detected", "Use the Detected Laser's Name and Size", ""),
    a(
        "device.bridge_setup",
        "Set Up a Laser Behind a Network Bridge",
        r#"{"type":"object","properties":{"host":{"type":"string"},"port":{"type":"integer"},"name":{"type":"string"},"width":{"type":"number"},"height":{"type":"number"}},"required":["host"]}"#,
    ),
    a("device.bridge_wizard", "Network Bridge Setup", ""),
    a("machine.bridge_rescan", "Rescan the Network Bridge", r#"{"type":"object","properties":{"host":{"type":"string"},"port":{"type":"integer"}}}"#),
    a("device.automation_status", "Automation Socket Status", ""),
    a("device.automation_open", "Automation Socket Settings", ""),
];

pub fn handles(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "device.set_connection" => &["CONN-01", "CONN-02", "CONN-03", "CONN-05", "CONN-07", "CONN-08", "GRBL-11"],
        "device.find" => &["DEV-02"],
        "device.detected" | "device.adopt_detected" => &["GRBL-12", "DEV-02"],
        "device.bridge_setup" | "device.bridge_wizard" => &["DEV-06"],
        "machine.bridge_rescan" => &["LASER-20"],
        "device.automation_status" | "device.automation_open" => &["CONN-09"],
        _ => &[],
    }
}

fn parse_connection(s: &str) -> Option<ConnectionKind> {
    use ConnectionKind as K;
    [K::Serial, K::Bluetooth, K::Tcp, K::Http, K::RawUsb, K::Udp, K::Usb, K::Bridge, K::BoardUsb]
        .into_iter()
        .find(|k| format!("{k:?}").eq_ignore_ascii_case(s) || k.label().eq_ignore_ascii_case(s))
}

fn parse_sim(s: &str) -> Option<SimFirmware> {
    SimFirmware::ALL.into_iter().find(|f| format!("{f:?}").eq_ignore_ascii_case(s) || f.label().eq_ignore_ascii_case(s))
}

fn set_connection(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let id = a.get("id").and_then(Value::as_u64).or(app.machine.devices.active).ok_or("no device is selected")?;
    let mut d = app.machine.devices.get(id).ok_or("no such device")?.clone();
    let text = |k: &str| a.get(k).and_then(Value::as_str);
    let int = |k: &str, max: u64| match a.get(k) {
        None => Ok(None),
        Some(v) => v.as_u64().filter(|n| *n <= max).map(Some).ok_or_else(|| format!("argument {k} must be a whole number up to {max}")),
    };
    if let Some(c) = text("connection") {
        let kind = parse_connection(c).ok_or_else(|| format!("unknown connection {c}"))?;
        if !d.kind.connections().contains(&kind) {
            let offered: Vec<&str> = d.kind.connections().iter().map(|k| k.label()).collect();
            return Err(format!("{} does not connect that way. It offers: {}.", d.kind.label(), offered.join(", ")));
        }
        d.connection = kind;
    }
    if let Some(h) = text("host") {
        d.host = h.trim().to_owned();
    }
    if let Some(p) = int("port", 65_535)? {
        match d.connection {
            ConnectionKind::Http => d.settings.conn.http_port = p as u16,
            ConnectionKind::Bridge => d.settings.conn.bridge_port = p as u16,
            _ => d.tcp_port = p as u16,
        }
    }
    if let Some(b) = int("baud", 4_000_000)? {
        d.settings.gcode.baud_rate = b as u32;
    }
    if let Some(p) = text("serial_port") {
        d.state.port = p.trim().to_owned();
    }
    if let Some(p) = text("http_path") {
        d.settings.conn.http_path = p.trim().to_owned();
    }
    if let Some(v) = text("usb_vid") {
        d.settings.conn.usb_vid = v.trim().to_owned();
    }
    if let Some(v) = text("usb_pid") {
        d.settings.conn.usb_pid = v.trim().to_owned();
    }
    if let Some(v) = int("usb_interface", 255)? {
        d.settings.conn.usb_interface = v as u8;
    }
    if let Some(v) = int("usb_ep_out", 15)? {
        d.settings.conn.usb_ep_out = v as u8;
    }
    if let Some(v) = int("usb_ep_in", 15)? {
        d.settings.conn.usb_ep_in = v as u8;
    }
    if let Some(v) = a.get("pulse_dtr").and_then(Value::as_bool) {
        d.settings.gcode.dtr = v;
    }
    if let Some(v) = a.get("dtr_enabled").and_then(Value::as_bool) {
        d.settings.conn.dtr_enabled = v;
    }
    if let Some(f) = text("sim_firmware") {
        if d.kind != DeviceType::SimulatorGrbl {
            return Err("Only the GRBL simulator has a firmware choice.".into());
        }
        d.settings.conn.sim_firmware = parse_sim(f).ok_or_else(|| format!("unknown firmware {f}"))?;
    }
    if let Some(e) = d.validate().into_iter().next() {
        return Err(e);
    }
    let link = d.link(&app.machine.ports).map(|l| l.label());
    app.machine.devices.update(d);
    if app.machine.devices.active == Some(id) {
        app.machine.applied_reset();
        crate::machine_ui::adopt_device_state(app);
    }
    Ok(json!({ "ok": true, "id": id, "link": link.ok() }))
}

fn bridge_rescan(app: &mut DarkBurn, a: &Value, src: Source) -> Result<Value, String> {
    let dev = app.machine.active().clone();
    let host = a.get("host").and_then(Value::as_str).map(str::to_owned).unwrap_or(dev.host.clone());
    let port = a.get("port").and_then(Value::as_u64).map_or(dev.settings.conn.bridge_port, |p| p as u16);
    let r = probe_bridge(&host, port, src);
    app.machine.conn.bridge_status = Some(match &r {
        Ok(m) => (m.clone(), true),
        Err(m) => (m.clone(), false),
    });
    r.map(|m| json!({ "ok": true, "message": m }))
}

/// Run one of the commands above. `None` when `id` is not one of them.
pub fn run(app: &mut DarkBurn, id: &str, a: &Value, src: Source) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    let null = json!({});
    let a = if a.is_null() { &null } else { a };
    Some(match id {
        "device.set_connection" => set_connection(app, a),
        "device.find" => {
            app.machine.rescan_ports();
            rescan_usb(app);
            let ports: Vec<Value> = crate::devices::find_my_laser(&app.machine.ports)
                .iter()
                .map(|c| json!({ "port": c.port, "label": c.label, "type": format!("{:?}", c.kind), "note": c.note }))
                .collect();
            let usb: Vec<Value> = app
                .machine
                .conn
                .usb_devices
                .iter()
                .map(|u| json!({ "vid": format!("{:04X}", u.vid), "pid": format!("{:04X}", u.pid), "label": u.label(), "serial": u.serial_number }))
                .collect();
            Ok(json!({ "ports": ports, "usb": usb, "note": "Only lists were read; nothing was opened." }))
        }
        "device.detected" => Ok(detected_json(app)),
        "device.adopt_detected" => adopt_detected(app),
        "device.bridge_setup" => (|| {
            let mut w = BridgeWizard { step: 2, ..Default::default() };
            w.host = a.get("host").and_then(Value::as_str).unwrap_or_default().to_owned();
            if let Some(p) = a.get("port").and_then(Value::as_u64).filter(|p| (1..=65_535).contains(p)) {
                w.port = p as u16;
            }
            if let Some(n) = a.get("name").and_then(Value::as_str) {
                w.name = n.to_owned();
            }
            w.width = a.get("width").and_then(Value::as_f64).unwrap_or(w.width);
            w.height = a.get("height").and_then(Value::as_f64).unwrap_or(w.height);
            let id = w.finish(&mut app.machine.devices)?;
            Ok(json!({ "ok": true, "id": id }))
        })(),
        "device.bridge_wizard" => {
            app.machine.conn.bridge = Some(BridgeWizard::default());
            Ok(json!({ "ok": true }))
        }
        "machine.bridge_rescan" => bridge_rescan(app, a, src),
        "device.automation_status" => {
            let s = &app.machine.devices.automation;
            let rt = &app.machine.conn.automation;
            Ok(json!({
                "enabled": s.enabled, "port": s.port, "reply_port": s.reply_port, "allow_remote": s.allow_remote,
                "listening_on": rt.local_port, "error": rt.error, "log": rt.log.iter().collect::<Vec<_>>(),
            }))
        }
        "device.automation_open" => {
            app.machine.conn.automation_open = true;
            Ok(json!({ "ok": true }))
        }
        other => Err(format!("{other} is not a connection command")),
    })
}

// ============================================================================================
// Drawing
// ============================================================================================

fn small(ui: &mut Ui, label: &str, tip: &str, enabled: bool) -> bool {
    ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, Button::Small).backing_on_hover_text(tip).clicked()).inner
}

fn note(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.text_dim));
}

/// The fields of the connection kinds in this file (Device Settings and the wizard).
pub fn connection_fields(ui: &mut Ui, d: &mut Device) {
    match d.connection {
        ConnectionKind::Bluetooth => {
            ui_kit::row(ui, "Baud rate", |ui| {
                ui_kit::number_field(ui, &mut d.settings.gcode.baud_rate, 1200..=2_000_000, 100.0, "");
            });
            note(
                ui,
                "Pair the laser in the system's Bluetooth settings first. It then shows up as a serial port; pick it in the Laser panel (automatic takes the first Bluetooth port).",
            );
        }
        ConnectionKind::Http => {
            ui_kit::row(ui, "Host", |ui| {
                ui_kit::field(ui, &mut d.host);
            });
            ui_kit::row(ui, "Port", |ui| {
                ui_kit::number_field(ui, &mut d.settings.conn.http_port, 1..=65_535, 1.0, "");
            });
            ui_kit::row(ui, "Request", |ui| {
                ui_kit::hint_field(ui, &mut d.settings.conn.http_path, "/command?plain=");
            });
            note(ui, "Each line is sent as one web request: the address above with the line added to the end.");
        }
        ConnectionKind::RawUsb => {
            ui_kit::row(ui, "Vendor ID (hex)", |ui| {
                ui_kit::hint_field(ui, &mut d.settings.conn.usb_vid, "1A86");
            });
            ui_kit::row(ui, "Product ID (hex)", |ui| {
                ui_kit::hint_field(ui, &mut d.settings.conn.usb_pid, "7523");
            });
            ui_kit::row(ui, "Serial number", |ui| {
                ui_kit::hint_field(ui, &mut d.usb_serial, "first one found");
            });
            ui_kit::row(ui, "Interface", |ui| {
                ui_kit::number_field(ui, &mut d.settings.conn.usb_interface, 0..=255, 1.0, "");
            });
            ui_kit::row(ui, "OUT endpoint", |ui| {
                ui_kit::number_field(ui, &mut d.settings.conn.usb_ep_out, 1..=15, 1.0, "");
            });
            ui_kit::row(ui, "IN endpoint", |ui| {
                ui_kit::number_field(ui, &mut d.settings.conn.usb_ep_in, 1..=15, 1.0, "");
            });
            note(ui, "For controllers with no serial port driver. Find My Laser lists the USB devices and fills in the IDs.");
            if !cfg!(feature = "rawusb") {
                note(ui, "This build has no raw USB support.");
            }
        }
        ConnectionKind::Bridge => {
            ui_kit::row(ui, "Bridge address", |ui| {
                ui_kit::field(ui, &mut d.host);
            });
            ui_kit::row(ui, "Port", |ui| {
                ui_kit::number_field(ui, &mut d.settings.conn.bridge_port, 1..=65_535, 1.0, "");
            });
            note(ui, "A network bridge sits next to the laser and passes DarkBurn's data on to it. Devices > Network bridge walks you through it.");
        }
        _ => {}
    }
}

/// The simulator's firmware choice (Device Settings, Basic page).
pub fn simulator_fields(ui: &mut Ui, d: &mut Device) {
    if d.kind != DeviceType::SimulatorGrbl {
        return;
    }
    let labels: Vec<&str> = SimFirmware::ALL.iter().map(|f| f.label()).collect();
    let mut idx = SimFirmware::ALL.iter().position(|f| *f == d.settings.conn.sim_firmware).unwrap_or(0);
    ui_kit::row(ui, "Simulated firmware", |ui| {
        ui_kit::dropdown(ui, "ds_sim_firmware", &mut idx, &labels);
    });
    d.settings.conn.sim_firmware = SimFirmware::ALL[idx];
    note(ui, "The simulator answers like this firmware the next time you connect.");
}

/// DTR control on the GCode page [GRBL-11].
pub fn dtr_fields(ui: &mut Ui, d: &mut Device) {
    ui_kit::checkbox(ui, "Keep DTR on while connected", &mut d.settings.conn.dtr_enabled);
}

/// Buttons on the Devices dialog.
pub fn devices_toolbar(ui: &mut Ui, app: &mut DarkBurn) {
    if small(ui, "Network bridge", "Set up a laser that sits behind a network bridge", true) {
        app.machine.conn.bridge = Some(BridgeWizard::default());
    }
    if small(ui, "Automation", "Let another program start jobs and ask for the status", true) {
        app.machine.conn.automation_open = true;
    }
}

/// The USB part of Find My Laser.
pub fn find_rows(ui: &mut Ui, app: &mut DarkBurn) {
    ui_kit::section_heading(ui, "USB devices");
    let list = app.machine.conn.usb_devices.clone();
    if list.is_empty() {
        note(ui, "No USB devices listed. Press Rescan after plugging the laser in.");
    }
    for (i, u) in list.iter().enumerate() {
        ui.push_id(("find_usb", i), |ui| {
            ui.horizontal(|ui| {
                ui.label(u.label());
                if small(ui, "Add as raw USB", "Add this device with a raw USB connection", true) {
                    app.machine.dlg.wizard = Some(wizard_from_usb(u));
                }
            });
        });
    }
    note(ui, "A laser behind a network bridge is added with Devices > Network bridge.");
}

/// Rows on the Laser panel: the detected laser and the bridge rescan.
pub fn laser_panel_rows(ui: &mut Ui, app: &mut DarkBurn) {
    let t = Tokens::get();
    let dev = app.machine.active().clone();
    if dev.connection == ConnectionKind::Bridge && !dev.is_simulator() {
        if ui_kit::button(ui, "Rescan bridge", Button::Small)
            .backing_on_hover_text("See whether the network bridge answers. This opens a connection to the bridge and closes it again.")
            .clicked()
        {
            crate::machine_ui::run_with(app, "machine.bridge_rescan", &json!({}));
        }
        if let Some((m, good)) = app.machine.conn.bridge_status.clone() {
            ui_kit::status_line(ui, &m, if good { StatusTone::Dim } else { StatusTone::Warning });
        }
    }
    if let Some(d) = app.machine.handle.snapshot().detected.clone() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("Detected laser: {}", d.label())).size(t.font_small).color(t.text_dim));
            if ui_kit::button(ui, "Use it", Button::Small)
                .backing_on_hover_text("Take the name, and the work area and S value the controller reports, into this device")
                .clicked()
            {
                crate::machine_ui::run_with(app, "device.adopt_detected", &json!({}));
            }
        });
    }
}

/// The dialogs of this file, once per frame.
pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    bridge_dialog(ctx, app);
    automation_dialog(ctx, app);
}

fn bridge_dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(mut w) = app.machine.conn.bridge.take() else { return };
    let last = w.step == BridgeWizard::STEPS.len() - 1;
    let ok = w.problems().is_empty();
    let mut test = false;
    let (closed, action) = ui_kit::Window::new(Id::unique("db_bridge_wizard"), "settings", "Network bridge setup")
        .cards(3.0)
        .fields()
        .actions(if last { "Add" } else { "Next" }, "Cancel", None)
        .show(ctx, ok, |ui| {
            ui_kit::steps(ui, "bridge_steps", &BridgeWizard::STEPS, w.step, None);
            match w.step {
                0 => {
                    note(
                        ui,
                        "A network bridge is a small computer next to the laser. DarkBurn talks to the bridge over your network, and the bridge passes everything on to the laser's controller. Type the bridge's address as shown on its screen or in your router.",
                    );
                    ui_kit::row(ui, "Bridge address", |ui| {
                        ui_kit::field(ui, &mut w.host);
                    });
                    ui_kit::row(ui, "Port", |ui| {
                        ui_kit::number_field(ui, &mut w.port, 1..=65_535, 1.0, "");
                    });
                    if small(ui, "Test the bridge", "Opens a connection to the bridge and closes it again", !w.host.trim().is_empty()) {
                        test = true;
                    }
                    if let Some((m, good)) = &w.tested {
                        ui_kit::status_line(ui, m, if *good { StatusTone::Dim } else { StatusTone::Warning });
                    }
                }
                1 => {
                    ui_kit::row(ui, "Name", |ui| {
                        ui_kit::field(ui, &mut w.name);
                    });
                    ui_kit::row(ui, "Width", |ui| {
                        ui_kit::number_field(ui, &mut w.width, 1.0..=10_000.0, 1.0, " mm");
                    });
                    ui_kit::row(ui, "Height", |ui| {
                        ui_kit::number_field(ui, &mut w.height, 1.0..=10_000.0, 1.0, " mm");
                    });
                }
                _ => {
                    for (k, v) in [
                        ("Name", w.name.clone()),
                        ("Type", DeviceType::Ruida.label().to_owned()),
                        ("Connection", format!("Network bridge at {}:{}", w.host.trim(), w.port)),
                        ("Work area", format!("{} x {} mm", w.width, w.height)),
                    ] {
                        ui_kit::row(ui, k, |ui| {
                            ui.label(v);
                        });
                    }
                    note(ui, "DarkBurn does not open the connection until you press Connect in the Laser panel.");
                }
            }
            for p in w.problems() {
                ui_kit::status_line(ui, &p, StatusTone::Error);
            }
            if w.step > 0 && small(ui, "Back", "", true) {
                w.step -= 1;
            }
        });
    if test {
        w.tested = Some(match probe_bridge(&w.host, w.port, Source::Ui) {
            Ok(m) => (m, true),
            Err(m) => (m, false),
        });
    }
    match action {
        Action::Ok if last => match w.finish(&mut app.machine.devices) {
            Ok(id) => {
                app.machine.dlg.devices_sel = Some(id);
                app.machine.say("Added the laser behind the bridge. Press Connect in the Laser panel when you are ready.");
            }
            Err(e) => {
                app.machine.dlg.message = Some(e);
                app.machine.conn.bridge = Some(w);
            }
        },
        Action::Ok => {
            w.step += 1;
            app.machine.conn.bridge = Some(w);
        }
        _ if closed || action == Action::Cancel => {}
        _ => app.machine.conn.bridge = Some(w),
    }
}

fn automation_dialog(ctx: &Context, app: &mut DarkBurn) {
    if !app.machine.conn.automation_open {
        return;
    }
    let (closed, action) =
        ui_kit::Window::new(Id::unique("db_automation"), "settings", "Automation socket").cards(3.0).fields().actions("", "Close", None).show(
            ctx,
            true,
            |ui| {
                note(
                    ui,
                    "Another program can send DarkBurn short text commands over UDP: PING, STATUS, START, PAUSE, RESUME and STOP. START runs the job exactly like the Start button, on whatever laser is connected.",
                );
                let s = &mut app.machine.devices.automation;
                ui_kit::checkbox(ui, "Listen for automation commands", &mut s.enabled);
                ui_kit::row(ui, "Listen on UDP port", |ui| {
                    ui_kit::number_field(ui, &mut s.port, 1..=65_535, 1.0, "");
                });
                ui_kit::row(ui, "Answer on UDP port", |ui| {
                    ui_kit::number_field(ui, &mut s.reply_port, 0..=65_535, 1.0, "");
                });
                note(ui, "0 answers the port the command came from.");
                ui_kit::checkbox(ui, "Accept commands from other computers", &mut s.allow_remote);
                if s.allow_remote {
                    ui_kit::status_line(ui, "Anyone on your network can then start the laser. Leave this off unless you need it.", StatusTone::Warning);
                }
                let rt = &app.machine.conn.automation;
                if let Some(e) = &rt.error {
                    ui_kit::status_line(ui, e, StatusTone::Error);
                } else if let Some(p) = rt.local_port {
                    ui_kit::status_line(ui, &format!("Listening on UDP port {p}."), StatusTone::Dim);
                } else {
                    note(ui, "Not listening.");
                }
                for l in rt.log.iter().rev().take(5) {
                    note(ui, l);
                }
            },
        );
    if closed || action != Action::None {
        app.machine.conn.automation_open = false;
    }
}
