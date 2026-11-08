//! The commands of the G-code firmwares, the galvo board and the Gweike file workflow that sit
//! beside the `machine.*` set in `machine_ui.rs`. `machine_ui::dispatch` hands them to [`run`].
//!
//! * `machine.*` here are hardware commands (they act on a connected machine). An agent reaches
//!   the simulators only; a real machine is refused by `machine_ui::dispatch` before [`run`].
//! * `gcode.*`, `galvo.*`, `gweike.*` and `device.*` work on the document and the device list
//!   and never touch a machine. The Gweike commands are flagged by [`gweike`] (they touch
//!   Gweike machine concepts) and the ones that write a file by [`writes_files`]; the MCP side
//!   reads both.
//! * There is no command here that connects to a Gweike card: the Gweike file workflow has no
//!   transport at all.

#![allow(dead_code, reason = "the MCP backend and some helpers use parts of this file")]

use std::path::Path;

use darkburn_core::kurbo::{Affine, BezPath, Point};
use darkburn_core::{Shape, layer};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::devices::{Device, DeviceType};
use crate::galvo_job::{self, FrameAdjust};
use crate::gcode_machines as gcm;
use crate::gweike_workflow::{self as gw, Head};
use crate::machine_api::{Family, MachineCommand, MachineError};
use crate::machine_ui::Source;
use crate::shell::DarkBurn;

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    // G-code controllers
    a("machine.air", "Air Assist On or Off", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#),
    a("machine.emergency_stop", "Emergency Stop (controller)", ""),
    a("machine.persist_settings", "Store the Settings in the Controller", ""),
    a(
        "machine.write_gcode_setting",
        "Write a Controller Setting by Name",
        r#"{"type":"object","properties":{"key":{"type":"string"},"value":{"type":"string"}},"required":["key","value"]}"#,
    ),
    a(
        "gcode.export",
        "Write the Job as G-code for a Firmware",
        r#"{"type":"object","properties":{"path":{"type":"string"},"device_type":{"type":"string","description":"GRBL, Smoothie, Marlin, Snapmaker, LinuxCnc, CustomGcode (default: the selected device)"}},"required":["path"]}"#,
    ),
    a("gcode.scripts", "Show the Start and End Scripts", r#"{"type":"object","properties":{"device_type":{"type":"string"}}}"#),
    a(
        "gcode.guess_firmware",
        "Guess the Firmware from a Banner",
        r#"{"type":"object","properties":{"banner":{"type":"string"}},"required":["banner"]}"#,
    ),
    a(
        "device.add",
        "Add a Device of a Type",
        r#"{"type":"object","properties":{"type":{"type":"string"},"name":{"type":"string"}},"required":["type"]}"#,
    ),
    a(
        "device.export_library",
        "Export a Device as a G-code Library Profile",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#,
    ),
    // Galvo
    a("machine.galvo_arm", "Arm or Disarm the Laser (galvo)", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#),
    a(
        "machine.galvo_frame",
        "Red-light Frame (galvo)",
        r#"{"type":"object","properties":{"repeat":{"type":"boolean"},"nudge_x":{"type":"number"},"nudge_y":{"type":"number"},"scale_pct":{"type":"number"},"hull":{"type":"boolean"}}}"#,
    ),
    a("machine.galvo_light", "Red Pointer On or Off (galvo)", r#"{"type":"object","properties":{"on":{"type":"boolean"}},"required":["on"]}"#),
    a(
        "machine.galvo_lens",
        "Load or Clear the Lens Correction (galvo)",
        r#"{"type":"object","properties":{"path":{"type":"string","description":"a .cor file; leave out to clear"}}}"#,
    ),
    a(
        "machine.galvo_axis",
        "Move an External Axis (galvo)",
        r#"{"type":"object","properties":{"axis":{"type":"integer"},"mm":{"type":"number"}},"required":["axis","mm"]}"#,
    ),
    a("galvo.estimate", "Compile the Galvo Job and Estimate its Time", ""),
    a("galvo.settings_get", "Read the Galvo Device Settings", ""),
    a(
        "galvo.settings_set",
        "Change Galvo Device Settings",
        r#"{"type":"object","properties":{"values":{"type":"object","description":"fields of the galvo settings to change"}},"required":["values"]}"#,
    ),
    a("galvo.import_ezcad", "Import an EZCad Configuration", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a(
        "galvo.lens_test",
        "Add the Lens Correction Test Pattern",
        r#"{"type":"object","properties":{"size_mm":{"type":"number"},"divisions":{"type":"integer"},"cross_mm":{"type":"number"},"grid_lines":{"type":"boolean"},"layer":{"type":"integer"}}}"#,
    ),
    a(
        "galvo.repeat_marking",
        "Set Repeat Marking",
        r#"{"type":"object","properties":{"count":{"type":"integer"},"axis_on":{"type":"boolean"},"axis":{"type":"integer"},"step_mm":{"type":"number"},"wait_foot":{"type":"boolean"}}}"#,
    ),
    // Gweike (files and the card simulator only)
    a("gweike.open", "Gweike Craft File and Card Simulator", ""),
    a(
        "gweike.layer_map",
        "Map a Layer to a Gweike Material and Craft",
        r#"{"type":"object","properties":{"layer":{"type":"integer"},"material":{"type":"string"},"head":{"enum":["fibre","co2","skip"]},"slot":{"type":"integer"}},"required":["layer"]}"#,
    ),
    a(
        "gweike.set_craft",
        "Set a Gweike Craft",
        r#"{"type":"object","properties":{"head":{"enum":["fibre","co2"]},"slot":{"type":"integer"},"craft":{"type":"object"}},"required":["head","slot","craft"]}"#,
    ),
    a("gweike.crafts", "Read the Gweike Craft Table", ""),
    a("gweike.export_chf", "Export the Gweike Craft File (.chf)", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a("gweike.plan", "Gweike Planner Dry Run", ""),
    a("gweike.simulate", "Run the Plan on the Gweike Card Simulator", ""),
];

/// Whether `id` is handled here.
pub fn handles(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id) || matches!(id, "laser.repeat_marking" | "laser.lens_test")
}

/// Commands here that act on a machine.
pub fn is_hardware(id: &str) -> bool {
    id.starts_with("machine.")
}

/// Commands here that only edit documents, files or the device list, though their id starts
/// with `laser.`.
pub fn not_hardware(id: &str) -> bool {
    matches!(id, "laser.repeat_marking" | "laser.lens_test")
}

/// The command touches Gweike machine concepts (the MCP side reads this).
pub fn gweike(id: &str) -> bool {
    id.starts_with("gweike.") || matches!(id, "file.export_gweike_dxf" | "file.export_gweike_nc")
}

/// The command writes a file.
pub fn writes_files(id: &str) -> bool {
    matches!(id, "gweike.export_chf" | "gcode.export" | "file.export_gweike_dxf" | "file.export_gweike_nc")
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "machine.air" => &["DS-12"],
        "machine.emergency_stop" => &["GCODE-01", "GCODE-02"],
        "machine.persist_settings" | "machine.write_gcode_setting" => &["GCODE-01", "GCODE-02"],
        "gcode.export" | "gcode.scripts" => &["DS-14", "GCODE-01", "GCODE-02", "GCODE-03", "GCODE-04", "GCODE-05"],
        "gcode.guess_firmware" => &["DEV-02"],
        "device.add" | "device.export_library" => &["DEV-03", "DEV-05"],
        "machine.galvo_arm" => &["GALVO-01", "LASER-21"],
        "machine.galvo_frame" => &["GALVO-09", "GALVO-10", "LASER-21"],
        "machine.galvo_light" => &["LASER-21"],
        "machine.galvo_lens" => &["GALVO-03", "AUX-09"],
        "machine.galvo_axis" => &["GALVO-15"],
        "galvo.estimate" => &["GALVO-01"],
        "galvo.settings_get" | "galvo.settings_set" => {
            &["DS-18", "GALVO-02", "GALVO-04", "GALVO-05", "GALVO-06", "GALVO-07", "GALVO-08", "GALVO-11", "GALVO-12"]
        }
        "galvo.import_ezcad" => &["GALVO-18"],
        "galvo.lens_test" | "laser.lens_test" => &["LT-18", "GALVO-17"],
        "galvo.repeat_marking" | "laser.repeat_marking" => &["LT-17", "GALVO-13", "GALVO-14"],
        "gweike.open" | "gweike.layer_map" | "gweike.set_craft" | "gweike.crafts" => &["MCC-04"],
        "gweike.export_chf" => &["MCC-02"],
        "gweike.plan" | "gweike.simulate" => &["MCC-05"],
        _ => &[],
    }
}

// ----- UI state -----------------------------------------------------------------------------------

/// Open dialogs and fields of the pieces in this file.
#[derive(Default)]
pub struct ExtUi {
    pub galvo_frame_open: bool,
    pub frame: FrameAdjust,
    pub galvo_lens_open: bool,
    pub lens_test: LensTest,
    pub galvo_repeat_open: bool,
    pub gweike_open: bool,
    pub gweike_plan: Option<gw::PlanSummary>,
    pub gweike_card: Option<gw::CardReport>,
    pub gweike_message: Option<String>,
    pub gweike_craft_head: Head,
    pub gweike_craft_slot: usize,
    pub gcode_scripts_open: bool,
    /// Machine Settings of a Smoothieware or Marlin controller: edits not written yet.
    pub gcode_edits: std::collections::BTreeMap<String, String>,
    /// The galvo Device Settings dialog's last EZCad import message.
    pub galvo_message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensTest {
    pub size_mm: f64,
    pub divisions: u32,
    pub cross_mm: f64,
    pub grid_lines: bool,
    pub layer: usize,
}

impl Default for LensTest {
    fn default() -> Self {
        LensTest { size_mm: 100.0, divisions: 10, cross_mm: 1.5, grid_lines: true, layer: 0 }
    }
}

// ----- Helpers ------------------------------------------------------------------------------------

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn text<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("argument {k} must be text"))
}

fn flag(a: &Value, k: &str) -> Result<bool, String> {
    a.get(k).and_then(Value::as_bool).ok_or_else(|| format!("argument {k} must be true or false"))
}

fn mach(r: Result<(), MachineError>) -> Result<Value, String> {
    r.map(|_| json!({ "ok": true })).map_err(|e| e.to_string())
}

/// A device type from its name (`Marlin`, `marlin`, a label).
pub fn parse_type(s: &str) -> Option<DeviceType> {
    DeviceType::ALL.iter().copied().find(|t| format!("{t:?}").eq_ignore_ascii_case(s) || t.label().eq_ignore_ascii_case(s)).or(
        match s.to_ascii_lowercase().as_str() {
            "smoothieware" => Some(DeviceType::Smoothie),
            "linuxcnc" => Some(DeviceType::LinuxCnc),
            "custom" => Some(DeviceType::CustomGcode),
            "gweike" => Some(DeviceType::GweikeCardSimulator),
            _ => None,
        },
    )
}

fn galvo_device(app: &DarkBurn) -> Result<Device, String> {
    let d = app.machine.active().clone();
    if d.family() == Family::Galvo { Ok(d) } else { Err("The selected device is not a galvo. Pick a galvo device first.".into()) }
}

fn parse_head(s: &str) -> Result<Head, String> {
    match s.to_ascii_lowercase().as_str() {
        "fibre" | "fiber" => Ok(Head::Fibre),
        "co2" => Ok(Head::Co2),
        "skip" | "none" => Ok(Head::Skip),
        other => Err(format!("unknown head {other}: use fibre, co2 or skip")),
    }
}

// ----- Dispatch -----------------------------------------------------------------------------------

/// Run one of the commands above. `None` when `id` is not one of them.
pub fn run(app: &mut DarkBurn, id: &str, a: &Value, _src: Source) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    let null = json!({});
    let a = if a.is_null() { &null } else { a };
    Some(match id {
        "machine.air" => flag(a, "on").and_then(|on| mach(app.machine.handle.command(MachineCommand::Air(on)))),
        "machine.emergency_stop" => mach(app.machine.handle.command(MachineCommand::EmergencyStop)),
        "machine.persist_settings" => mach(app.machine.handle.command(MachineCommand::PersistSettings)),
        "machine.write_gcode_setting" => (|| {
            let (k, v) = (text(a, "key")?.to_owned(), text(a, "value")?.to_owned());
            mach(app.machine.handle.command(MachineCommand::WriteGcodeSetting { key: k, value: v }))
        })(),
        "gcode.export" => gcode_export(app, a),
        "gcode.scripts" => gcode_scripts(app, a),
        "gcode.guess_firmware" => text(a, "banner").map(|b| {
            let g = gcm::guess_type(b);
            json!({ "device_type": g.map(|t| format!("{t:?}")), "label": g.map(DeviceType::label) })
        }),
        "device.add" => device_add(app, a),
        "device.export_library" => (|| {
            let id = a.get("id").and_then(Value::as_u64).ok_or("argument id must be a whole number")?;
            let d = app.machine.devices.get(id).ok_or("no such device")?;
            Ok(json!({ "text": gcm::export_library_profile(d) }))
        })(),
        "machine.galvo_arm" => flag(a, "on").and_then(|on| mach(app.machine.handle.command(MachineCommand::GalvoArm(on)))),
        "machine.galvo_frame" => galvo_frame(app, a),
        "machine.galvo_light" => flag(a, "on").and_then(|on| mach(app.machine.handle.command(MachineCommand::GalvoLight(on)))),
        "machine.galvo_lens" => galvo_lens(app, a),
        "machine.galvo_axis" => (|| {
            let axis = a.get("axis").and_then(Value::as_u64).filter(|v| *v < 2).ok_or("argument axis must be 0 or 1")? as u8;
            let mm = num(a, "mm").ok_or("argument mm must be a number")?;
            mach(app.machine.handle.command(MachineCommand::GalvoAxis { axis, mm }))
        })(),
        "galvo.estimate" => galvo_device(app).and_then(|d| {
            let b = galvo_job::build_job(&app.editor.doc, &d)?;
            let c = galvo_job::compile(&b.job, &d)?;
            let mut v = galvo_job::estimate_json(&c);
            v["warnings"] = json!(b.warnings);
            Ok(v)
        }),
        "galvo.settings_get" => galvo_device(app).map(|d| serde_json::to_value(&d.settings.galvo).unwrap_or(Value::Null)),
        "galvo.settings_set" => galvo_settings_set(app, a),
        "galvo.import_ezcad" => galvo_import(app, a),
        "galvo.lens_test" | "laser.lens_test" => lens_test(app, a, id == "laser.lens_test"),
        "galvo.repeat_marking" | "laser.repeat_marking" => repeat_marking(app, a, id == "laser.repeat_marking"),
        "gweike.open" => {
            app.machine.ext.gweike_open = true;
            Ok(json!({ "ok": true, "note": gw::DIRECT_CONTROL_NOTE }))
        }
        "gweike.layer_map" => gweike_layer_map(app, a),
        "gweike.set_craft" => gweike_set_craft(app, a),
        "gweike.crafts" => {
            let s = &app.machine.active().settings.gweike;
            Ok(json!({ "crafts": s.crafts, "layers": s.layers, "problems": s.craft_problems() }))
        }
        "gweike.export_chf" => gweike_export(app, a),
        "gweike.plan" => gweike_plan(app, false),
        "gweike.simulate" => gweike_plan(app, true),
        other => Err(format!("{other} is not a machine command")),
    })
}

// ----- G-code -------------------------------------------------------------------------------------

fn gcode_export(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let path = Path::new(text(a, "path")?).to_path_buf();
    let dev = match a.get("device_type").and_then(Value::as_str) {
        Some(t) => {
            let ty = parse_type(t).ok_or_else(|| format!("unknown device type {t}"))?;
            if !ty.selectable() {
                return Err(ty.needs_capture().unwrap_or("not available").to_owned());
            }
            Device::new(ty)
        }
        None => app.machine.active().clone(),
    };
    let job = crate::job_cmds::build_now(app)?;
    let text = gcm::job_text(&dev, &job, None, &dev.settings.gcode);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({ "ok": true, "path": path.display().to_string(), "lines": text.lines().count(), "device_type": format!("{:?}", dev.kind) }))
}

fn gcode_scripts(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let dev = match a.get("device_type").and_then(Value::as_str) {
        Some(t) => Device::new(parse_type(t).ok_or_else(|| format!("unknown device type {t}"))?),
        None => app.machine.active().clone(),
    };
    let (start, end) = gcm::scripts_of(&dev);
    Ok(json!({ "start": start, "end": end, "device_type": format!("{:?}", dev.kind) }))
}

fn device_add(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let t = text(a, "type")?;
    let ty = parse_type(t).ok_or_else(|| format!("unknown device type {t}"))?;
    if let Some(why) = ty.needs_capture() {
        return Err(why.to_owned());
    }
    let mut d = Device::new(ty);
    if let Some(n) = a.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()) {
        d.name = n.trim().to_owned();
    }
    let id = app.machine.devices.add(d);
    Ok(json!({ "ok": true, "id": id }))
}

// ----- Galvo --------------------------------------------------------------------------------------

/// The Start button for a galvo: compile the job from the document and hand it to the machine.
/// The laser still needs the user's arm; the board refuses to start otherwise.
pub fn galvo_start(app: &mut DarkBurn, send_only: bool) -> Result<Value, String> {
    if send_only {
        return Err("A galvo marks as it is sent. Use Start.".into());
    }
    let dev = galvo_device(app)?;
    let (req, warnings) = galvo_job::request(&app.editor.doc, &dev, false, false)?;
    let armed = app.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed);
    if !armed {
        return Err("Arm the laser first (Arm in the Laser panel). The laser is never enabled without it.".into());
    }
    match app.machine.begin_start(req)? {
        crate::machine_ui::StartOutcome::Started => Ok(json!({ "ok": true, "warnings": warnings })),
        crate::machine_ui::StartOutcome::AskChecklist => Ok(json!({ "ok": true, "waiting_for_checklist": true })),
    }
}

fn galvo_frame(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let dev = galvo_device(app)?;
    let mut adj = app.machine.ext.frame;
    if let Some(v) = num(a, "nudge_x") {
        adj.nudge_mm.0 = v;
    }
    if let Some(v) = num(a, "nudge_y") {
        adj.nudge_mm.1 = v;
    }
    if let Some(v) = num(a, "scale_pct") {
        adj.scale_pct = v;
    }
    if let Some(v) = a.get("hull").and_then(Value::as_bool) {
        adj.hull = v;
    }
    if let Some(v) = a.get("repeat").and_then(Value::as_bool) {
        adj.repeat = v;
    }
    let b = galvo_job::build_job(&app.editor.doc, &dev)?;
    let frame = galvo_job::frame_for(&b.job, &dev, &adj)?;
    app.machine.ext.frame = adj;
    mach(app.machine.handle.command(MachineCommand::GalvoFrame { frame: Box::new(frame), repeat: adj.repeat }))
}

fn galvo_lens(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let dev = galvo_device(app)?;
    let id = dev.id;
    let path = a.get("path").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let table = if path.is_empty() {
        None
    } else {
        let mut probe = dev.settings.galvo.clone();
        probe.cor_path = path.clone();
        probe.load_lens()?
    };
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.galvo.cor_path = path.clone();
    }
    match app.machine.handle.command(MachineCommand::GalvoLens(table.map(Box::new))) {
        Ok(()) => Ok(json!({ "ok": true, "loaded": !path.is_empty() })),
        // Not connected: it is loaded when the board connects.
        Err(MachineError::NotConnected) => Ok(json!({ "ok": true, "loaded": !path.is_empty(), "note": "It is sent to the board when it connects." })),
        Err(e) => Err(e.to_string()),
    }
}

fn galvo_settings_set(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let dev = galvo_device(app)?;
    let values = a.get("values").and_then(Value::as_object).ok_or("argument values must be an object")?;
    let mut cur = serde_json::to_value(&dev.settings.galvo).map_err(|e| e.to_string())?;
    for (k, v) in values {
        if cur.get(k).is_none() {
            return Err(format!("no galvo setting called {k}"));
        }
        match (cur.get_mut(k), v) {
            (Some(Value::Object(old)), Value::Object(new)) => {
                for (k2, v2) in new {
                    old.insert(k2.clone(), v2.clone());
                }
            }
            (Some(slot), v) => *slot = v.clone(),
            _ => {}
        }
    }
    let s: crate::galvo_settings::GalvoDeviceSettings = serde_json::from_value(cur).map_err(|e| format!("galvo settings: {e}"))?;
    if let Some(e) = s.validate().first() {
        return Err(e.clone());
    }
    let id = dev.id;
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.galvo = s;
    }
    Ok(json!({ "ok": true }))
}

fn galvo_import(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let dev = galvo_device(app)?;
    let path = text(a, "path")?;
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
    let imp = darkburn_galvo::ezcad::import(&bytes).map_err(|e| e.to_string())?;
    let id = dev.id;
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.galvo.apply_library(&imp.settings);
        if let Some(c) = &imp.cor_file {
            d.settings.galvo.cor_path = c.clone();
        }
    }
    Ok(json!({ "ok": true, "applied": imp.applied, "ignored": imp.ignored, "warnings": imp.warnings, "cor_file": imp.cor_file }))
}

fn lens_test(app: &mut DarkBurn, a: &Value, from_menu: bool) -> Result<Value, String> {
    if from_menu {
        app.machine.ext.galvo_lens_open = true;
        return Ok(json!({ "ok": true, "dialog": "lens test" }));
    }
    let mut p = app.machine.ext.lens_test;
    if let Some(v) = num(a, "size_mm") {
        p.size_mm = v;
    }
    if let Some(v) = a.get("divisions").and_then(Value::as_u64) {
        p.divisions = v.clamp(1, 50) as u32;
    }
    if let Some(v) = num(a, "cross_mm") {
        p.cross_mm = v.max(0.0);
    }
    if let Some(v) = a.get("grid_lines").and_then(Value::as_bool) {
        p.grid_lines = v;
    }
    if let Some(v) = a.get("layer").and_then(Value::as_u64) {
        p.layer = (v as usize).min(layer::COLOUR_LAYERS - 1);
    }
    let bed = app.machine.active().bed();
    if !(p.size_mm > 0.0 && p.size_mm <= bed.0.min(bed.1)) {
        return Err(format!("The pattern size must be between 0 and the field size ({} mm).", bed.0.min(bed.1)));
    }
    let mut path = BezPath::new();
    for sub in galvo_job::lens_test_paths(p.size_mm, p.divisions, p.cross_mm, p.grid_lines) {
        path.extend(sub.elements().iter().copied());
    }
    path.apply_affine(Affine::translate((bed.0 / 2.0, bed.1 / 2.0)));
    let n = path.elements().len();
    app.machine.ext.lens_test = p;
    let ok = app.editor.edit("Lens correction test pattern", |d, _| {
        d.add(Shape::path(path, p.layer));
        true
    });
    if ok { Ok(json!({ "ok": true, "elements": n, "layer": p.layer })) } else { Err("Could not add the pattern.".into()) }
}

fn repeat_marking(app: &mut DarkBurn, a: &Value, from_menu: bool) -> Result<Value, String> {
    if from_menu {
        app.machine.ext.galvo_repeat_open = true;
        return Ok(json!({ "ok": true, "dialog": "repeat marking" }));
    }
    let dev = galvo_device(app)?;
    let mut r = dev.settings.galvo.repeat;
    if let Some(v) = a.get("count").and_then(Value::as_u64) {
        r.count = (v as u32).clamp(1, 100_000);
    }
    if let Some(v) = a.get("axis_on").and_then(Value::as_bool) {
        r.axis_on = v;
    }
    if let Some(v) = a.get("axis").and_then(Value::as_u64) {
        r.axis = (v as u8).min(1);
    }
    if let Some(v) = num(a, "step_mm") {
        r.axis_step_mm = v;
    }
    if let Some(v) = a.get("wait_foot").and_then(Value::as_bool) {
        r.wait_foot_each = v;
    }
    let id = dev.id;
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.galvo.repeat = r;
    }
    Ok(json!({ "ok": true, "count": r.count }))
}

// ----- Gweike -------------------------------------------------------------------------------------

fn gweike_layer_map(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let layer = a.get("layer").and_then(Value::as_u64).filter(|l| *l < layer::COLOUR_LAYERS as u64).ok_or("argument layer must be 0 to 29")? as usize;
    let id = app.machine.active().id;
    let cur = app.machine.active().settings.gweike.map_of(layer, &app.editor.doc);
    let mut m = cur;
    if let Some(s) = a.get("material").and_then(Value::as_str) {
        m.material = s.trim().to_owned();
    }
    if let Some(s) = a.get("head").and_then(Value::as_str) {
        m.head = parse_head(s)?;
    }
    if let Some(v) = a.get("slot").and_then(Value::as_u64) {
        if v as usize >= darkburn_mcc::craft::LAYERS_PER_HEAD {
            return Err(format!("The craft slot must be 0 to {}.", darkburn_mcc::craft::LAYERS_PER_HEAD - 1));
        }
        m.slot = v as u8;
    }
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.gweike.layers.insert(layer, m);
    }
    Ok(json!({ "ok": true }))
}

fn gweike_set_craft(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let head = parse_head(text(a, "head")?)?;
    let slot = a.get("slot").and_then(Value::as_u64).ok_or("argument slot must be a whole number")? as usize;
    let craft = a.get("craft").ok_or("argument craft is missing")?;
    let id = app.machine.active().id;
    let mut s = app.machine.active().settings.gweike.clone();
    gw::set_craft(&mut s, head, slot, craft)?;
    let problems = s.craft_problems();
    if let Some(d) = app.machine.devices.get_mut(id) {
        d.settings.gweike = s;
    }
    Ok(json!({ "ok": true, "problems": problems }))
}

fn gweike_export(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let path = Path::new(text(a, "path")?).to_path_buf();
    let s = app.machine.active().settings.gweike.clone();
    let (b, side) = gw::export_chf(&app.editor.doc, &s, &path)?;
    Ok(json!({
        "ok": true, "path": path.display().to_string(), "crafts_file": side.display().to_string(),
        "contours": b.contours, "skipped_layers": b.skipped_layers, "warnings": b.warnings,
        "craft_problems": s.craft_problems(), "note": gw::DIRECT_CONTROL_NOTE,
    }))
}

fn gweike_plan(app: &mut DarkBurn, simulate: bool) -> Result<Value, String> {
    let dev = app.machine.active().clone();
    let (plan, sum) = gw::plan(&app.editor.doc, &dev.settings.gweike, dev.bed())?;
    app.machine.ext.gweike_plan = Some(sum.clone());
    let mut out = json!({
        "ticks": sum.ticks, "frames": sum.frames, "bytes": sum.bytes, "estimated_s": sum.estimated_s, "cut_s": sum.cut_s,
        "rapid_s": sum.rapid_s, "cut_length_mm": sum.cut_length_mm, "rapid_length_mm": sum.rapid_length_mm,
        "simulator_only": true,
    });
    if simulate {
        let r = gw::run_on_card_simulator(&plan)?;
        out["card"] = json!({
            "frames_accepted": r.frames_accepted, "ticks_consumed": r.ticks_consumed, "underruns": r.underruns,
            "pulse_position": [r.pulse_position.0, r.pulse_position.1], "forbidden_attempts": r.forbidden_attempts,
            "laser_refusals": r.laser_refusals, "sim_seconds": r.sim_seconds,
        });
        app.machine.ext.gweike_card = Some(r);
    }
    Ok(out)
}

/// Where a point of the design lands on the galvo field (for tests and the panel).
pub fn field_point(p: Point, bed: (f64, f64)) -> Point {
    crate::galvo_machine::to_field(p, bed)
}
