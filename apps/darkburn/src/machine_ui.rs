//! The machine side of the app window: state, the registered `machine.*` and `device.*`
//! commands, and the once-per-frame work (poll the machine, save the device list, guard closing).
//! The panels and dialogs that draw it are in `laser_panel.rs` and `devices_ui.rs`.
//!
//! Every button calls [`dispatch`] with an id from [`COMMANDS`] and JSON arguments, the same call
//! an agent makes through `api::execute`. All `machine.*` commands are `hardware: true`:
//!
//! * The UI path ([`Source::Ui`]) can reach a real machine only through the Connect confirmation
//!   dialog, which shows the "REAL MACHINE" banner afterwards. Nothing connects at start-up.
//! * The agent path ([`Source::Api`]) works on the simulators only. A real machine is refused.

#![allow(dead_code, reason = "some helpers are used only by the dialogs and tests")]

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use darkburn_grbl::PortInfo;
use darkburn_job::Corner;
use darkburn_ruida as ruida;
use kurbo::{Point, Rect};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::devices::{Candidate, ConnectionKind, Device, DeviceStore, DeviceType, Wizard};
use crate::machine_api::{
    ConnectSpec, Family, FrameSpec, JobRequest, JogSpec, LaserState, MachineCommand, MachineError, MachineEvent, MachineHandle, MoveSpec,
    RealLinkConsent, SettingsBackup,
};
use crate::shell::DarkBurn;

// ----- The commands -------------------------------------------------------------------------

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

/// Registered in `commands::registry()`. `machine.*` are hardware commands; `device.*` only edit
/// the device list.
pub const COMMANDS: &[Cmd] = &[
    a("machine.connect", "Connect to the Laser", r#"{"type":"object","properties":{"device":{"type":"integer"}}}"#),
    a("machine.disconnect", "Disconnect from the Laser", ""),
    a("machine.reset_connection", "Reset the Connection", ""),
    a("machine.status", "Machine Status", ""),
    a("machine.start", "Start the Job", ""),
    a("machine.pause", "Pause the Job", ""),
    a("machine.resume", "Resume the Job", ""),
    a("machine.stop", "Stop the Job", ""),
    a(
        "machine.frame",
        "Frame a Rectangle",
        r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"width":{"type":"number"},"height":{"type":"number"},"feed":{"type":"number"},"laser_percent":{"type":"number"}},"required":["x","y","width","height"]}"#,
    ),
    a("machine.home", "Home", ""),
    a("machine.unlock", "Unlock After an Alarm", ""),
    a("machine.go_to_origin", "Go to Origin", ""),
    a(
        "machine.jog",
        "Jog",
        r#"{"type":"object","properties":{"dx":{"type":"number"},"dy":{"type":"number"},"dz":{"type":"number"},"du":{"type":"number"},"feed":{"type":"number"}}}"#,
    ),
    a(
        "machine.jog_start",
        "Start a Continuous Jog",
        r#"{"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"z":{"type":"integer"},"u":{"type":"integer"},"feed":{"type":"number"}}}"#,
    ),
    a("machine.jog_stop", "Stop the Continuous Jog", ""),
    a(
        "machine.move_to",
        "Move to a Position",
        r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"z":{"type":"number"},"feed":{"type":"number"},"machine_coords":{"type":"boolean"}}}"#,
    ),
    a("machine.set_origin", "Set the User Origin Here", ""),
    a("machine.clear_origin", "Clear the User Origin", ""),
    a("machine.get_position", "Get the Position", ""),
    a("machine.fire", "Fire the Laser Briefly", r#"{"type":"object","properties":{"percent":{"type":"number"},"secs":{"type":"number"}}}"#),
    a("machine.fire_off", "Laser Off", ""),
    a(
        "machine.overrides",
        "Set Speed, Power or Rapid Overrides",
        r#"{"type":"object","properties":{"feed":{"type":"integer"},"power":{"type":"integer"},"rapid":{"type":"integer"}}}"#,
    ),
    a("machine.console", "Send a Console Line", r#"{"type":"object","properties":{"line":{"type":"string"}},"required":["line"]}"#),
    a("machine.macro", "Run a Macro", r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#),
    a("machine.read_settings", "Read Machine Settings", ""),
    a(
        "machine.write_setting",
        "Write a Machine Setting",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"value":{"type":"string"}},"required":["id","value"]}"#,
    ),
    a("machine.file_list", "List Controller Files", ""),
    a("machine.file_run", "Run a Controller File", r#"{"type":"object","properties":{"index":{"type":"integer"}},"required":["index"]}"#),
    a(
        "machine.file_delete",
        "Delete Controller Files",
        r#"{"type":"object","properties":{"indices":{"type":"array","items":{"type":"integer"}}},"required":["indices"]}"#,
    ),
    a("machine.file_delete_all", "Delete All Controller Files", ""),
    a(
        "machine.file_calc_time",
        "Calculate a Controller File's Time",
        r#"{"type":"object","properties":{"index":{"type":"integer"}},"required":["index"]}"#,
    ),
    a("machine.controller_info", "Read Controller Info", ""),
    a("machine.show_all", "Show Every Sent Line in the Console", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#),
    a(
        "machine.file_upload",
        "Send a .rd File to the Controller",
        r#"{"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"},"start":{"type":"boolean"}},"required":["path"]}"#,
    ),
    a(
        "machine.file_overwrite",
        "Answer the Overwrite Question",
        r#"{"type":"object","properties":{"choice":{"enum":["overwrite","rename","cancel"]}},"required":["choice"]}"#,
    ),
    a(
        "machine.dsp_read",
        "Read Laser 2 or U Axis Settings",
        r#"{"type":"object","properties":{"what":{"enum":["laser2","u_axis"]}},"required":["what"]}"#,
    ),
    a(
        "machine.dsp_write",
        "Write Laser 2 or U Axis Settings",
        r#"{"type":"object","properties":{"what":{"enum":["laser2","u_axis"]},"values":{"type":"object"}},"required":["what","values"]}"#,
    ),
    a("device.list", "List Devices", ""),
    a("device.select", "Choose the Device", r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#),
    a("device.duplicate", "Duplicate a Device", r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#),
    a("device.remove", "Remove a Device", r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#),
    a("device.export", "Export a Device Profile", r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#),
    a("device.import", "Import a Device Profile", r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#),
    a("laser.devices", "Devices", ""),
];

/// Whether `id` is handled here.
pub fn handles(id: &str) -> bool {
    crate::machine_job::handles(id)
        || id.starts_with("machine.")
        || id.starts_with("device.")
        || matches!(id, "laser.devices" | "laser.controller_info" | "edit.device_settings" | "edit.machine_settings")
}

/// The commands here that do not touch a machine (and so are not flagged hardware).
pub fn is_hardware(id: &str) -> bool {
    crate::machine_job::handles(id)
        || id.starts_with("machine.")
        || matches!(id, "laser.devices" | "laser.controller_info" | "edit.device_settings" | "edit.machine_settings")
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "job.start" => &["LASER-03", "LASER-24"],
        "job.pause" | "job.stop" => &["LASER-03"],
        "job.frame" => &["LASER-04"],
        "job.frame_rubber" => &["LASER-05"],
        "job.frame_selection" => &["LASER-04", "LASER-14"],
        "job.send" => &["LASER-08"],
        "job.run_gcode" => &["LASER-07"],
        "job.home" => &["LASER-09"],
        "job.go_origin" => &["LASER-10"],
        "job.preview_run_from_here" | "job.preview_send_from_here" => &["PREV-09"],
        "machine.connect" | "machine.disconnect" | "machine.reset_connection" | "laser.devices" => &["LASER-01", "LASER-17"],
        "machine.start" | "machine.pause" | "machine.resume" | "machine.stop" => &["LASER-03", "RUIDA-03"],
        "machine.frame" => &["LASER-04", "LASER-05"],
        "machine.home" => &["LASER-09", "MOVE-04"],
        "machine.go_to_origin" => &["LASER-10"],
        "machine.jog" | "machine.jog_start" | "machine.jog_stop" => &["MOVE-01", "MOVE-06", "GRBL-06", "RUIDA-04"],
        "machine.move_to" => &["MOVE-09"],
        "machine.set_origin" | "machine.clear_origin" => &["MOVE-13"],
        "machine.get_position" => &["MOVE-08"],
        "machine.fire" | "machine.fire_off" => &["MOVE-15", "GRBL-10"],
        "machine.overrides" => &["MOVE-17", "GRBL-09"],
        "machine.console" | "machine.macro" => &["CON-01", "CON-03", "CON-04"],
        "machine.read_settings" | "machine.write_setting" | "edit.machine_settings" => &["EDIT-31", "GRBL-07", "RUIDA-07"],
        "machine.file_list" | "machine.file_run" | "machine.file_delete" | "machine.file_delete_all" => &["FL-01", "FL-02", "RUIDA-06"],
        "machine.file_calc_time" => &["FL-03"],
        "machine.controller_info" | "laser.controller_info" => &["LT-16", "RUIDA-08"],
        "machine.show_all" => &["CON-02"],
        "machine.file_upload" | "machine.file_overwrite" => &["FL-02", "RUIDA-05"],
        "machine.dsp_read" | "machine.dsp_write" => &["RUIDA-09", "RUIDA-10", "MOVE-18"],
        "edit.device_settings" => &["EDIT-30"],
        "device.list" | "device.select" | "device.duplicate" | "device.remove" | "device.export" | "device.import" => &["DEV-01", "DEV-04", "DEV-05"],
        _ => &[],
    }
}

// ----- State ---------------------------------------------------------------------------------

/// A line of feedback under the panels.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// A remembered position [MOVE-10].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedPos {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub use_z: bool,
}

#[derive(Clone, Debug)]
pub struct JogState {
    /// mm
    pub distance: f64,
    /// mm/s
    pub speed: f64,
    pub z_speed: f64,
    pub continuous: bool,
    pub invert_keypad: bool,
    pub goto_x: String,
    pub goto_y: String,
    pub goto_z: String,
    pub from_machine_zero: bool,
    pub saved: Vec<SavedPos>,
    pub finish: Option<(f64, f64)>,
    /// A continuous jog is under way (a direction button is held).
    pub holding: bool,
}

impl Default for JogState {
    fn default() -> Self {
        JogState {
            distance: 10.0,
            speed: 100.0,
            z_speed: 5.0,
            continuous: false,
            invert_keypad: false,
            goto_x: "0".into(),
            goto_y: "0".into(),
            goto_z: String::new(),
            from_machine_zero: false,
            saved: Vec::new(),
            finish: None,
            holding: false,
        }
    }
}

/// Open dialogs.
#[derive(Default)]
pub struct Dialogs {
    pub devices: bool,
    pub devices_sel: Option<u64>,
    pub wizard: Option<Wizard>,
    pub find: Option<Vec<Candidate>>,
    /// Device Settings: the draft and the page (0 Basic, 1 GCode, 2 DSP, 3 Additional).
    pub settings: Option<(Device, usize)>,
    pub machine_settings: bool,
    pub controller_info: bool,
    /// A real connection waits for the user's confirmation.
    pub confirm_connect: Option<ConnectSpec>,
    pub confirm_close: bool,
    /// The pre-job checklist: the job and one tick per item.
    pub checklist: Option<(Box<JobRequest>, Vec<bool>)>,
    pub macro_edit: Option<(usize, String)>,
    pub message: Option<String>,
    /// Machine Settings: edits not written yet (GRBL `$N` -> text, Ruida address -> text).
    pub setting_edits: std::collections::BTreeMap<u32, String>,
    pub backup_name: String,
    pub confirm_restore: Option<SettingsBackup>,
    pub laser2: Option<ruida::settings::Laser2Settings>,
    pub u_axis: Option<ruida::settings::UAxisSettings>,
}

pub struct MachineUi {
    pub handle: MachineHandle,
    pub devices: DeviceStore,
    pub ports: Vec<PortInfo>,
    pub ports_scanned: bool,
    loaded: bool,
    saved: String,
    pub message: Option<Message>,
    pub dlg: Dialogs,
    pub jog: JogState,
    pub console_input: String,
    pub show_all: bool,
    pub fire_percent: f64,
    pub fire_secs: f64,
    pub file_sel: BTreeSet<u16>,
    pub upload_name: String,
    /// Closing is allowed although a job runs (the user said so).
    pub close_ok: bool,
    pub rotary_on: bool,
    pub cylinder_on: bool,
    /// The machine events of the last frame, for tests and the log.
    pub last_events: Vec<MachineEvent>,
    /// When the running job began, for the "elapsed" display of machines that do not report it.
    pub job_log: Vec<String>,
    /// The device whose state is applied to the app, to notice a switch.
    applied_device: Option<u64>,
    /// What the job builder was last given from the active device (see [`sync_job_state`]).
    job_synced: Option<(u64, crate::job_build::DeviceGcodeSettings, crate::job_build::AdditionalSettings, crate::scan_offsets::ScanTuning)>,
}

impl Default for MachineUi {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineUi {
    pub fn new() -> Self {
        MachineUi {
            handle: MachineHandle::new(),
            devices: DeviceStore::default(),
            ports: Vec::new(),
            ports_scanned: false,
            loaded: false,
            saved: String::new(),
            message: None,
            dlg: Dialogs::default(),
            jog: JogState::default(),
            console_input: String::new(),
            show_all: false,
            fire_percent: 1.0,
            fire_secs: 0.5,
            file_sel: BTreeSet::new(),
            upload_name: "DARKBURN".into(),
            close_ok: false,
            rotary_on: false,
            cylinder_on: false,
            last_events: Vec::new(),
            job_log: Vec::new(),
            applied_device: None,
            job_synced: None,
        }
    }

    pub fn active(&self) -> &Device {
        // The store always has a valid active device (see DeviceStore::sanitise).
        self.devices.active().unwrap_or_else(|| &self.devices.devices[0])
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), error: false, at: Instant::now() });
    }
    pub fn fail(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), error: true, at: Instant::now() });
    }

    /// Load the device list from the settings folder, once.
    pub fn ensure_loaded(&mut self, dir: Option<&Path>, safe_mode: bool) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let (Some(dir), false) = (dir, safe_mode) {
            self.devices = DeviceStore::load(dir);
        }
        self.saved = self.devices.to_json();
    }

    /// Write the device list when it changed.
    pub fn save_if_changed(&mut self, dir: Option<&Path>, safe_mode: bool) {
        if !self.loaded || safe_mode {
            return;
        }
        let now = self.devices.to_json();
        if now != self.saved {
            if let Some(dir) = dir
                && let Err(e) = self.devices.save(dir)
            {
                log::warn!("could not save the device list: {e}");
                return;
            }
            self.saved = now;
        }
    }

    /// The serial ports, listed (never opened) [LASER-18].
    pub fn rescan_ports(&mut self) {
        self.ports = darkburn_grbl::list_ports();
        self.ports_scanned = true;
    }

    /// Start connecting the active device. A simulator connects at once; a real device waits for
    /// the user's confirmation in the dialog.
    pub fn request_connect(&mut self) -> Result<(), String> {
        if !self.ports_scanned {
            self.rescan_ports();
        }
        let dev = self.active().clone();
        if dev.is_design_only() {
            return Err("This device is design only; there is no laser to connect.".into());
        }
        let spec = dev.connect_spec(&self.ports)?;
        if spec.link.is_simulator() {
            self.connect_now(spec, None)
        } else {
            self.dlg.confirm_connect = Some(spec);
            Ok(())
        }
    }

    /// The user confirmed the real connection.
    pub fn confirm_connect(&mut self) {
        if let Some(spec) = self.dlg.confirm_connect.take() {
            let consent = RealLinkConsent::from_user_click();
            if let Err(e) = self.connect_now(spec, Some(consent)) {
                self.fail(e);
            }
        }
    }

    fn connect_now(&mut self, spec: ConnectSpec, consent: Option<RealLinkConsent>) -> Result<(), String> {
        self.close_ok = false;
        self.handle.connect(&spec, consent).map_err(|e| e.to_string())?;
        self.show_all_sync();
        Ok(())
    }

    fn show_all_sync(&mut self) {
        // Applied after the handshake; remembered here.
    }

    /// Everything that must happen once per frame, with no window: poll the machine and turn its
    /// events into messages.
    pub fn tick(&mut self) -> Vec<MachineEvent> {
        let events = self.handle.poll();
        for e in &events {
            match e {
                MachineEvent::Connected(fw) => self.say(format!("Connected: {fw}")),
                MachineEvent::ConnectFailed(m) => self.fail(format!("Could not connect: {m}")),
                MachineEvent::Disconnected(Some(r)) => self.fail(format!("Connection lost: {r}")),
                MachineEvent::Alarm(m) => self.fail(format!("Alarm: {m}")),
                MachineEvent::Error(m) => self.fail(m.clone()),
                MachineEvent::JobStarted => self.job_log.push("Job started".into()),
                MachineEvent::JobFinished(p) => {
                    self.say(format!("Job finished in {}", format_duration(p.elapsed)));
                    self.job_log.push(format!("Job finished in {}", format_duration(p.elapsed)));
                }
                MachineEvent::JobAborted(r) => {
                    self.say(format!("Job stopped: {r}"));
                    self.job_log.push(format!("Job stopped: {r}"));
                }
                MachineEvent::SettingWritten { id, ok, message } => {
                    if *ok {
                        self.say(format!("Wrote {message}"));
                    } else {
                        self.fail(format!("{id}: {message}"));
                    }
                }
                MachineEvent::Done { what, ok: false, message } => self.fail(format!("{what}: {message}")),
                _ => {}
            }
        }
        self.last_events = events.clone();
        events
    }

    /// The `Start` button's first half: the pre-job checklist when the device wants one.
    pub fn begin_start(&mut self, job: JobRequest) -> Result<StartOutcome, String> {
        let dev = self.active();
        if dev.settings.checklist_on && !dev.settings.checklist.is_empty() {
            let n = dev.settings.checklist.len();
            self.dlg.checklist = Some((Box::new(job), vec![false; n]));
            return Ok(StartOutcome::AskChecklist);
        }
        self.start_now(job).map(|_| StartOutcome::Started)
    }

    /// The checklist was completed and confirmed.
    pub fn finish_checklist(&mut self) -> Result<(), String> {
        let Some((job, ticks)) = self.dlg.checklist.take() else { return Err("There is no checklist waiting.".into()) };
        if ticks.iter().any(|t| !t) {
            self.dlg.checklist = Some((job, ticks));
            return Err("Tick every item first.".into());
        }
        self.start_now(*job)
    }

    fn start_now(&mut self, job: JobRequest) -> Result<(), String> {
        self.handle.start_job(job).map_err(|e| e.to_string())
    }

    /// Window close guard [LASER-28]: true when the close must wait for the user.
    pub fn guard_close(&mut self) -> bool {
        if self.handle.job_running() && !self.close_ok {
            self.dlg.confirm_close = true;
            true
        } else {
            false
        }
    }

    /// The user chose "Stop and close".
    pub fn stop_and_close(&mut self) {
        let _ = self.handle.stop();
        self.close_ok = true;
        self.dlg.confirm_close = false;
    }

    /// Make the next frame adopt the active device's state again (after it was edited).
    pub fn applied_reset(&mut self) {
        self.applied_device = None;
    }

    /// Switch device: remember the old one's state, adopt the new one's [DEV-07].
    pub fn select_device(&mut self, id: u64) -> bool {
        if !self.devices.select(id) {
            return false;
        }
        if self.handle.is_connected() || self.handle.is_real_machine() {
            self.handle.disconnect();
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    AskChecklist,
}

pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

// ----- Jog helpers [MOVE-01, MOVE-07, MOVE-16] -----------------------------------------------------

/// The multiplier the modifier keys put on a jog step: Ctrl 10x, Shift 0.1x, both 0.01x.
pub fn jog_multiplier(ctrl: bool, shift: bool) -> f64 {
    match (ctrl, shift) {
        (false, false) => 1.0,
        (true, false) => 10.0,
        (false, true) => 0.1,
        (true, true) => 0.01,
    }
}

/// What an arrow on screen (right and up are positive) means in machine axes for a machine whose
/// zero is at `origin`: a zero at the top makes +Y go down the screen, a zero on the right makes
/// +X go left. `invert_keypad` flips the vertical arrows.
pub fn arrow_to_axes(dir_x: i8, dir_y: i8, origin: Corner, invert_keypad: bool) -> (i8, i8) {
    let sx = if matches!(origin, Corner::BottomRight | Corner::TopRight) { -1 } else { 1 };
    let mut sy = if matches!(origin, Corner::TopLeft | Corner::TopRight) { -1 } else { 1 };
    if invert_keypad {
        sy = -sy;
    }
    (dir_x * sx, dir_y * sy)
}

// ----- Commands ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A button in the window.
    Ui,
    /// An agent or script.
    Api,
}

/// Run command `id`, returning `None` when it is not one of ours.
pub fn dispatch(app: &mut DarkBurn, id: &str, a: &Value, src: Source) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    // Keep the state fresh before acting on it.
    app.machine.tick();
    crate::machine_job::update_head(app);
    if src == Source::Api && is_hardware(id) && app.machine.handle.is_real_machine() {
        return Some(Err(format!("{id} was refused: a real machine is connected. Use the window; agents only drive the simulators.")));
    }
    Some(run_command(app, id, a, src))
}

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn need(a: &Value, k: &str) -> Result<f64, String> {
    num(a, k).ok_or_else(|| format!("argument {k} must be a number"))
}

fn mach(r: Result<(), MachineError>) -> Result<Value, String> {
    r.map(|_| json!({ "ok": true })).map_err(|e| e.to_string())
}

fn index_of(a: &Value, k: &str) -> Result<u16, String> {
    a.get(k).and_then(Value::as_u64).and_then(|v| u16::try_from(v).ok()).ok_or_else(|| format!("argument {k} must be a whole number"))
}

/// The job commands and the origin commands, which need the job state as well as the machine.
fn run_job_command(app: &mut DarkBurn, id: &str, a: &Value, src: Source) -> Option<Result<Value, String>> {
    use crate::machine_job::{self as mj, Action};
    let op = |a: &Value| a.get("op_index").and_then(Value::as_u64).map(|v| v as usize);
    let start = |app: &mut DarkBurn, action: Action| match src {
        Source::Ui => mj::start_later(app, action),
        Source::Api => mj::start_now(app, action, a.get("allow_out_of_bounds").and_then(Value::as_bool).unwrap_or(false)),
    };
    Some(match id {
        "job.start" | "machine.start" => {
            let from_op = op(a).or(app.job.start_op);
            start(app, Action::Start { from_op })
        }
        "job.preview_run_from_here" => match op(a) {
            Some(n) => start(app, Action::Start { from_op: Some(n) }),
            None => Err("argument op_index must be a whole number".into()),
        },
        "job.send" => {
            let from_op = app.job.start_op;
            start(app, Action::Send { from_op })
        }
        "job.preview_send_from_here" => match op(a) {
            Some(n) => start(app, Action::Send { from_op: Some(n) }),
            None => Err("argument op_index must be a whole number".into()),
        },
        "job.pause" => {
            if app.machine.handle.snapshot().state == LaserState::Paused {
                mach(app.machine.handle.resume())
            } else {
                mach(app.machine.handle.pause())
            }
        }
        "job.stop" => mach(app.machine.handle.stop()),
        "job.frame" | "machine.frame_job" => mj::frame(app, crate::job_build::FrameKind::Rectangle),
        "job.frame_rubber" => mj::frame(app, crate::job_build::FrameKind::RubberBand),
        "job.frame_selection" => mj::frame(app, crate::job_build::FrameKind::Selection),
        "job.home" => mach(app.machine.handle.home()),
        "job.go_origin" => mach(app.machine.handle.go_to_origin()),
        "job.run_gcode" => run_gcode_file(app, a, src),
        "machine.set_origin" => {
            let r = mach(app.machine.handle.set_user_origin());
            if r.is_ok() {
                mj::origin_changed(app, true);
            }
            r
        }
        "machine.clear_origin" => {
            let r = mach(app.machine.handle.clear_user_origin());
            if r.is_ok() {
                mj::origin_changed(app, false);
            }
            r
        }
        _ => return None,
    })
}

/// Run a saved G-code file on a G-code machine [LASER-07].
fn run_gcode_file(app: &mut DarkBurn, a: &Value, src: Source) -> Result<Value, String> {
    if !app.machine.handle.is_connected() {
        return Err("The laser is not connected. Press Connect in the Laser panel first.".into());
    }
    let path = match (a.get("path").and_then(Value::as_str), src) {
        (Some(p), _) => std::path::PathBuf::from(p),
        (None, Source::Ui) => match rfd::FileDialog::new().add_filter("G-code", crate::job_build::GCODE_EXTENSIONS).pick_file() {
            Some(p) => p,
            None => return Ok(json!({ "ok": true, "cancelled": true })),
        },
        (None, Source::Api) => return Err("argument path must be text".into()),
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut job = JobRequest::gcode(text.lines().map(str::to_owned).collect());
    job.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "gcode".into());
    if app.machine.handle.snapshot().family != Family::Grbl {
        return Err("Run G-code is for G-code machines.".into());
    }
    match app.machine.begin_start(job)? {
        StartOutcome::Started => Ok(json!({ "ok": true })),
        StartOutcome::AskChecklist => Ok(json!({ "ok": true, "waiting_for_checklist": true })),
    }
}

fn run_command(app: &mut DarkBurn, id: &str, a: &Value, src: Source) -> Result<Value, String> {
    if let Some(r) = run_job_command(app, id, a, src) {
        return r;
    }
    let m = &mut app.machine;
    match id {
        // ----- dialogs
        "laser.devices" => {
            m.dlg.devices = true;
            Ok(json!({ "ok": true }))
        }
        "edit.device_settings" => {
            m.dlg.settings = Some((m.active().clone(), 0));
            Ok(json!({ "ok": true }))
        }
        "edit.machine_settings" => {
            m.dlg.machine_settings = true;
            if m.handle.is_connected() {
                let _ = m.handle.read_settings();
            }
            Ok(json!({ "ok": true }))
        }
        "laser.controller_info" => {
            m.dlg.controller_info = true;
            if m.handle.is_connected() && m.handle.snapshot().family == Family::Ruida {
                let _ = m.handle.command(MachineCommand::ReadControllerInfo);
            }
            Ok(json!({ "ok": true }))
        }
        // ----- connection
        "machine.connect" => {
            if let Some(dev) = a.get("device").and_then(Value::as_u64)
                && !m.select_device(dev)
            {
                return Err(format!("no device {dev}"));
            }
            if m.active().is_design_only() {
                return Err("This device is design only; there is no laser to connect.".into());
            }
            if src == Source::Api && !m.active().is_simulator() {
                return Err("An agent may only connect to a simulator. Connect real machines from the Laser panel.".into());
            }
            m.request_connect()?;
            Ok(json!({ "ok": true, "waiting_for_confirmation": m.dlg.confirm_connect.is_some() }))
        }
        "machine.disconnect" => {
            m.handle.disconnect();
            Ok(json!({ "ok": true }))
        }
        "machine.reset_connection" => {
            m.handle.disconnect();
            m.request_connect()?;
            Ok(json!({ "ok": true, "waiting_for_confirmation": m.dlg.confirm_connect.is_some() }))
        }
        "machine.status" => Ok(status_json(m)),
        // ----- job
        "machine.pause" => mach(m.handle.pause()),
        "machine.resume" => mach(m.handle.resume()),
        "machine.stop" => mach(m.handle.stop()),
        "machine.frame" => {
            let r = Rect::new(need(a, "x")?, need(a, "y")?, need(a, "x")? + need(a, "width")?, need(a, "y")? + need(a, "height")?);
            let spec = FrameSpec { feed: num(a, "feed").unwrap_or(3000.0), laser_percent: num(a, "laser_percent") };
            mach(m.handle.frame_rect(r, spec))
        }
        "machine.home" => mach(m.handle.home()),
        "machine.unlock" => mach(m.handle.command(MachineCommand::Unlock)),
        "machine.go_to_origin" => mach(m.handle.go_to_origin()),
        // ----- moves
        "machine.jog" => {
            let spec = JogSpec {
                dx: num(a, "dx").unwrap_or(0.0),
                dy: num(a, "dy").unwrap_or(0.0),
                dz: num(a, "dz").unwrap_or(0.0),
                du: num(a, "du").unwrap_or(0.0),
                feed: num(a, "feed").unwrap_or(m.jog.speed * 60.0),
            };
            mach(m.handle.jog_step(spec))
        }
        "machine.jog_start" => {
            let d = |k: &str| a.get(k).and_then(Value::as_i64).unwrap_or(0).clamp(-1, 1) as i8;
            m.jog.holding = true;
            mach(m.handle.jog_start([d("x"), d("y"), d("z"), d("u")], num(a, "feed").unwrap_or(m.jog.speed * 60.0)))
        }
        "machine.jog_stop" => {
            m.jog.holding = false;
            mach(m.handle.jog_stop())
        }
        "machine.move_to" => {
            let spec = MoveSpec {
                x: num(a, "x"),
                y: num(a, "y"),
                z: num(a, "z"),
                machine_coords: a.get("machine_coords").and_then(Value::as_bool).unwrap_or(false),
                feed: num(a, "feed").unwrap_or(m.jog.speed * 60.0),
            };
            mach(m.handle.move_to(spec))
        }
        "machine.get_position" => mach(m.handle.get_position()),
        "machine.fire" => {
            let percent = num(a, "percent").unwrap_or(m.fire_percent);
            let secs = num(a, "secs").unwrap_or(m.fire_secs);
            mach(m.handle.fire(percent, secs))
        }
        "machine.fire_off" => mach(m.handle.fire_off()),
        "machine.overrides" => {
            let g = |k: &str| a.get(k).and_then(Value::as_u64).and_then(|v| u16::try_from(v).ok());
            mach(m.handle.set_overrides(g("feed"), g("power"), g("rapid")))
        }
        // ----- console and settings
        "machine.console" => mach(m.handle.send_console(a.get("line").and_then(Value::as_str).ok_or("argument line must be text")?)),
        "machine.macro" => mach(m.handle.run_macro(a.get("text").and_then(Value::as_str).ok_or("argument text must be text")?)),
        "machine.read_settings" => mach(m.handle.read_settings()),
        "machine.write_setting" => {
            let sid = a.get("id").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or("argument id must be a whole number")?;
            mach(m.handle.write_setting(sid, a.get("value").and_then(Value::as_str).ok_or("argument value must be text")?))
        }
        // ----- controller files
        "machine.file_list" => mach(m.handle.command(MachineCommand::ListFiles)),
        "machine.file_run" => mach(m.handle.command(MachineCommand::RunFile(index_of(a, "index")?))),
        "machine.file_delete" => {
            let v: Vec<u16> = a
                .get("indices")
                .and_then(Value::as_array)
                .ok_or("argument indices must be a list")?
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|v| u16::try_from(v).ok())
                .collect();
            mach(m.handle.command(MachineCommand::DeleteFiles(v)))
        }
        "machine.file_delete_all" => mach(m.handle.command(MachineCommand::DeleteAllFiles)),
        "machine.file_calc_time" => mach(m.handle.command(MachineCommand::CalcTime(index_of(a, "index")?))),
        "machine.controller_info" => mach(m.handle.command(MachineCommand::ReadControllerInfo)),
        "machine.show_all" => {
            let on = a.get("on").and_then(Value::as_bool).ok_or("argument on must be true or false")?;
            m.show_all = on;
            mach(m.handle.command(MachineCommand::ShowAll(on)))
        }
        "machine.file_upload" => {
            let path = std::path::PathBuf::from(a.get("path").and_then(Value::as_str).ok_or("argument path must be text")?);
            let data = std::fs::read(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
            let name = a
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| m.upload_name.clone());
            let mut job = JobRequest::new(crate::machine_api::JobData::RdFile { name: name.clone(), data });
            job.name = name;
            job.start_immediately = a.get("start").and_then(Value::as_bool).unwrap_or(false);
            job.auto_increment_name = m.active().settings.dsp.filename_increment;
            mach(m.handle.start_job(job))
        }
        "machine.file_overwrite" => {
            let c = match a.get("choice").and_then(Value::as_str) {
                Some("overwrite") => ruida::OverwriteChoice::Overwrite,
                Some("rename") => ruida::OverwriteChoice::Rename,
                Some("cancel") => ruida::OverwriteChoice::Cancel,
                _ => return Err("choice must be overwrite, rename or cancel".into()),
            };
            mach(m.handle.answer_overwrite(c))
        }
        "machine.dsp_read" => match a.get("what").and_then(Value::as_str) {
            Some("laser2") => mach(m.handle.command(MachineCommand::ReadLaser2)),
            Some("u_axis") => mach(m.handle.command(MachineCommand::ReadUAxis)),
            _ => Err("what must be laser2 or u_axis".into()),
        },
        "machine.dsp_write" => {
            let v = a.get("values").cloned().ok_or("argument values is missing")?;
            match a.get("what").and_then(Value::as_str) {
                Some("laser2") => {
                    mach(m.handle.command(MachineCommand::WriteLaser2(serde_json::from_value(v).map_err(|e| format!("laser 2 values: {e}"))?)))
                }
                Some("u_axis") => {
                    mach(m.handle.command(MachineCommand::WriteUAxis(serde_json::from_value(v).map_err(|e| format!("U axis values: {e}"))?)))
                }
                _ => Err("what must be laser2 or u_axis".into()),
            }
        }
        // ----- the device list
        "device.list" => Ok(json!({
            "active": m.devices.active,
            "devices": m.devices.devices.iter().map(|d| json!({
                "id": d.id, "name": d.name, "type": d.kind.label(), "simulator": d.is_simulator(),
                "width": d.width_mm, "height": d.height_mm,
            })).collect::<Vec<_>>(),
        })),
        "device.select" => {
            let did = a.get("id").and_then(Value::as_u64).ok_or("argument id must be a whole number")?;
            if m.select_device(did) { Ok(json!({ "active": did })) } else { Err(format!("no device {did}")) }
        }
        "device.duplicate" => {
            let did = a.get("id").and_then(Value::as_u64).ok_or("argument id must be a whole number")?;
            m.devices.duplicate(did).map(|n| json!({ "id": n })).ok_or_else(|| format!("no device {did}"))
        }
        "device.remove" => {
            let did = a.get("id").and_then(Value::as_u64).ok_or("argument id must be a whole number")?;
            if m.devices.remove(did) { Ok(json!({ "ok": true })) } else { Err("That device is not there, or it is the last one.".into()) }
        }
        "device.export" => {
            let did = a.get("id").and_then(Value::as_u64).ok_or("argument id must be a whole number")?;
            m.devices.export_profile(did).map(|t| json!({ "text": t })).ok_or_else(|| format!("no device {did}"))
        }
        "device.import" => {
            let text = a.get("text").and_then(Value::as_str).ok_or("argument text must be text")?;
            m.devices.import_profile(text).map(|n| json!({ "id": n }))
        }
        other => Err(format!("{other} is not a machine command")),
    }
}

pub fn status_json(m: &MachineUi) -> Value {
    let s = m.handle.snapshot();
    json!({
        "device": m.active().name,
        "simulator": s.simulator,
        "real_machine": m.handle.is_real_machine(),
        "link": format!("{:?}", s.link),
        "state": s.state.label(),
        "firmware": s.firmware,
        "port": s.port,
        "work_pos": s.work_pos,
        "machine_pos": s.machine_pos,
        "job": s.job.as_ref().map(|j| json!({
            "fraction": j.fraction, "elapsed_s": j.elapsed.as_secs_f64(), "remaining_s": j.remaining.map(|r| r.as_secs_f64()),
            "lines": j.lines_done, "total_lines": j.total_lines,
        })),
        "files": s.ruida.files.as_ref().map(|f| f.iter().map(|x| json!({"index": x.index, "name": x.name})).collect::<Vec<_>>()),
        "alarm": s.alarm,
        "fire_on": s.fire_on,
        "overrides": [s.overrides.0, s.overrides.1, s.overrides.2],
    })
}

/// A UI click on a command id (menus, ribbon, buttons). Errors become the panel message.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    run_with(app, id, &json!({}))
}

/// [`run`] with arguments built from the panel's fields.
pub fn run_with(app: &mut DarkBurn, id: &str, args: &Value) -> bool {
    match dispatch(app, id, args, Source::Ui) {
        None => false,
        Some(Ok(_)) => true,
        Some(Err(e)) => {
            app.machine.fail(e);
            true
        }
    }
}

/// The agent path: `None` when `id` is not ours.
pub fn api(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    dispatch(app, id, args, Source::Api)
}

// ----- Per frame -----------------------------------------------------------------------------

/// Once per frame: load/save the device list, poll the machine, adopt the active device's
/// remembered state, guard the window close. Dialogs are drawn by `devices_ui::dialogs`.
pub fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    let dir = app.settings_dir.clone();
    let safe = app.safe_mode;
    app.machine.ensure_loaded(dir.as_deref(), safe);
    adopt_device_state(app);
    if let Some(id) = app.machine.applied_device {
        // What the user changes while on a device is the device's from now on.
        capture_device_state(app, id);
    }
    sync_job_state(app);
    crate::machine_job::update_head(app);
    let events = app.machine.tick();
    crate::settings_wiring::on_machine_events(app, &events);
    let excluded = app.prefs.opts.excluded_ports.clone();
    crate::settings_wiring::filter_ports(&mut app.machine.ports, &excluded);
    if !events.is_empty() || app.machine.handle.snapshot().state.is_working() || app.machine.handle.snapshot().state == LaserState::Connecting {
        ctx.request_repaint();
    }
    if app.machine.handle.is_connected() {
        // Keep status moving in the panels without input.
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    if ctx.input(|i| i.viewport().close_requested()) && app.machine.guard_close() {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
    // Fix a held continuous jog if the pointer is gone.
    if app.machine.jog.holding && !ctx.input(|i| i.pointer.any_down()) {
        let _ = run_with(app, "machine.jog_stop", &json!({}));
    }
    app.machine.save_if_changed(dir.as_deref(), safe);
}

/// The corner as the canvas's origin setting.
pub fn view_origin(c: Corner) -> crate::view::Origin {
    use crate::view::Origin;
    match c {
        Corner::BottomLeft => Origin::FrontLeft,
        Corner::BottomRight => Origin::FrontRight,
        Corner::TopLeft => Origin::RearLeft,
        Corner::TopRight => Origin::RearRight,
    }
}

/// Put the active device's remembered state into the app when the device changed [DEV-07], and
/// remember the old device's first.
pub fn adopt_device_state(app: &mut DarkBurn) {
    let Some(active) = app.machine.devices.active else { return };
    if app.machine.applied_device == Some(active) {
        return;
    }
    if let Some(old) = app.machine.applied_device {
        capture_device_state(app, old);
    }
    app.machine.applied_device = Some(active);
    let Some(dev) = app.machine.devices.get(active).cloned() else { return };
    let remembered = dev.state.panel_layout.is_some();
    let p = &mut app.prefs;
    if remembered {
        p.numeric_inches = dev.state.inches;
        p.show_grid = dev.state.show_grid;
    }
    p.page_width = dev.width_mm as f32;
    p.page_height = dev.height_mm as f32;
    p.origin = view_origin(dev.origin);
    if let Some(layout) = dev.state.panel_layout.as_deref()
        && let Ok(panels) = serde_json::from_str::<std::collections::BTreeMap<String, crate::prefs::PanelState>>(layout)
    {
        for (id, st) in panels {
            if p.panels.contains_key(&id) {
                p.panels.insert(id, st);
            }
        }
    }
    p.sanitise();
    // Each device keeps its own material and cut library [LIB-09, LAYER-10].
    app.extras.library.switch_device(&dev.name, &app.prefs);
    if !remembered {
        // A device that has never been used takes over the app's current units, grid and panels.
        capture_device_state(app, active);
    }
    app.machine.rotary_on = false;
    app.machine.cylinder_on = false;
}

/// Copy what the app holds for the current device into the device record.
pub fn capture_device_state(app: &mut DarkBurn, id: u64) {
    let (inches, grid, layout) = (app.prefs.numeric_inches, app.prefs.show_grid, serde_json::to_string(&app.prefs.panels).ok());
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.state.inches = inches;
        d.state.show_grid = grid;
        d.state.panel_layout = layout;
    }
}

/// Store the project's layers as the device's defaults, or apply them [DEV-07].
pub fn save_default_layers(app: &mut DarkBurn) -> bool {
    let json = serde_json::to_string(&app.editor.doc.layers).ok();
    let Some(id) = app.machine.devices.active else { return false };
    match (json, app.machine.devices.get_mut(id)) {
        (Some(j), Some(d)) => {
            d.state.default_layers = Some(j);
            true
        }
        _ => false,
    }
}

pub fn apply_default_layers(app: &mut DarkBurn) -> bool {
    let Some(text) = app.machine.active().state.default_layers.clone() else { return false };
    let Ok(layers) = serde_json::from_str::<Vec<darkburn_core::layer::Layer>>(&text) else { return false };
    if layers.len() != app.editor.doc.layers.len() {
        return false;
    }
    app.editor.edit("Apply device default layers", |d, _| {
        d.layers = layers;
        true
    })
}

/// A device that can connect needs the connection page of the wizard.
pub fn needs_connection(kind: DeviceType) -> bool {
    !matches!(kind.connections(), [ConnectionKind::None])
}

/// Where a point on screen (design mm, Y up) lands on the machine; used by the frame buttons.
pub fn design_to_machine(p: Point, dev: &Device) -> Point {
    let x = if matches!(dev.origin, Corner::BottomRight | Corner::TopRight) { dev.width_mm - p.x } else { p.x };
    let y = if matches!(dev.origin, Corner::TopLeft | Corner::TopRight) { dev.height_mm - p.y } else { p.y };
    Point::new(x, y)
}

pub fn family_of(m: &MachineUi) -> Family {
    let s = m.handle.snapshot();
    if m.handle.is_connected() || s.state == LaserState::Connecting { s.family } else { m.active().family() }
}

/// The selected device profile is the source of truth for what the job builder uses: bed, origin
/// corner, G-code settings, motion numbers and scan offsets flow from it into `app.job`. Edits made
/// in the job builder's own G-code window or the images window flow back into the profile.
pub fn sync_job_state(app: &mut DarkBurn) {
    let Some(active) = app.machine.devices.active else { return };
    let scan_now = crate::scan_offsets::current();
    let same_device = app.machine.job_synced.as_ref().is_some_and(|s| s.0 == active);
    if same_device {
        let (_, g, ad, sc) = app.machine.job_synced.clone().unwrap_or_else(|| unreachable!());
        let job_changed = app.job.gcode != g || app.job.additional != ad || scan_now != sc;
        if job_changed && let Some(dev) = app.machine.devices.get_mut(active) {
            dev.settings.gcode = app.job.gcode.clone();
            dev.settings.additional = app.job.additional;
            dev.settings.scan = scan_now.clone();
        }
    }
    let Some(dev) = app.machine.devices.get(active) else { return };
    let (g, ad, sc) = (dev.settings.gcode.clone(), dev.settings.additional, dev.settings.scan.clone());
    let jd = dev.job_device();
    if app.job.gcode != g {
        app.job.gcode = g.clone();
    }
    if app.job.additional != ad {
        app.job.additional = ad;
    }
    if scan_now != sc {
        crate::scan_offsets::set(sc.clone());
    }
    app.job.device = jd;
    app.machine.job_synced = Some((active, g, ad, sc));
}

/// A `.dbdev` file was opened or dropped: add it to the device list and show it [DEV-05].
pub fn import_profile_file(app: &mut DarkBurn, path: &Path) {
    match std::fs::read_to_string(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))
        .and_then(|t| app.machine.devices.import_profile(&t))
    {
        Ok(id) => {
            app.machine.dlg.devices = true;
            app.machine.dlg.devices_sel = Some(id);
            let name = app.machine.devices.get(id).map(|d| d.name.clone()).unwrap_or_default();
            app.machine.say(format!("Added the device {name}"));
        }
        Err(e) => {
            app.machine.dlg.devices = true;
            app.machine.dlg.message = Some(e);
        }
    }
}
