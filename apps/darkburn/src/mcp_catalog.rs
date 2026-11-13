//! The command catalog as MCP sees it: every registered command with its safety class.
//!
//! The registry (`commands.rs`) knows `hardware` only as a yes/no. The safety gate needs more:
//! what kind of hardware effect (connect, move, configure, fire or start a job, stop), whether the
//! command drives a Gweike machine, and whether it writes files. Those answers are decided here,
//! in tables that a test walks over the whole registry, so a new command cannot slip in with a
//! wrong classification: an unclassified hardware command defaults to the most restrictive class
//! (`FireOrJob`, which also needs the user's confirmation), and a command that looks like it
//! saves or exports must be listed as writing files or as explicitly not writing them.

use darkburn_mcp::{CommandInfo, HardwareClass};
use serde_json::Value;

use crate::commands;
use crate::shell::DarkBurn;

/// Always allowed, whatever the policy says: they stop things.
const STOP: &[&str] =
    &["machine.pause", "machine.stop", "machine.emergency_stop", "job.pause", "job.stop", "machine.fire_off", "machine.jog_stop", "camera.stop"];

/// Start a job, resume it, run a file or raw lines, or fire the laser.
const FIRE_OR_JOB: &[&str] = &[
    "job.start",
    "job.send",
    "job.run_gcode",
    "job.preview_run_from_here",
    "job.preview_send_from_here",
    "machine.start",
    "machine.resume",
    "machine.fire",
    "machine.file_run",
    "machine.console",
    "machine.macro",
];

/// Open, close, scan or read a device (no motion, nothing written to it).
const CONNECT: &[&str] = &[
    "camera.find",
    "camera.start",
    "machine.connect",
    "machine.disconnect",
    "machine.reset_connection",
    "machine.status",
    "machine.controller_info",
    "machine.read_settings",
    "machine.file_list",
    "machine.file_calc_time",
    "machine.dsp_read",
    "machine.get_position",
    "machine.show_all",
    "laser.devices",
    "laser.controller_info",
];

/// Write settings or files to a controller, or change a running job's overrides.
const CONFIG: &[&str] = &[
    "machine.write_setting",
    "machine.dsp_write",
    "machine.file_delete",
    "machine.file_delete_all",
    "machine.file_upload",
    "machine.file_overwrite",
    "machine.overrides",
    "machine.set_finish",
    "machine.clear_finish",
    "edit.device_settings",
    "edit.machine_settings",
];

/// Move axes (or read where the head is to place the design there).
const MOTION: &[&str] = &[
    "machine.home",
    "machine.unlock",
    "machine.go_to_origin",
    "machine.jog",
    "machine.jog_start",
    "machine.move_to",
    "machine.set_origin",
    "machine.clear_origin",
    "machine.focus_z",
    "machine.jog_rotary",
    "machine.rotary_test",
    "machine.frame",
    "job.frame",
    "job.frame_rubber",
    "job.frame_selection",
    "job.home",
    "job.go_origin",
    "tools.position_laser",
    "arrange.to_laser",
];

/// Change the machine's setup without talking to it: which device is the active one (selecting
/// another also drops the open connection), where a device connects, the device list, and what
/// every job sends (start and end script, S range, motion limits, galvo timings). That is the
/// machine owner's, not the design's, so it needs the same permission as writing settings to a
/// controller.
const DEVICE_SETUP: &[&str] = &[
    "device.select",
    "device.add",
    "device.duplicate",
    "device.remove",
    "device.import",
    "device.set_connection",
    "device.bridge_setup",
    "device.adopt_detected",
    "device.read_motion",
    "job.set_gcode_settings",
    "galvo.settings_set",
    "galvo.import_ezcad",
    "galvo.repeat_marking",
];

/// Whether the command acts on a connected machine: moves axes, fires the laser, sends a job,
/// writes controller settings or talks to a device. The registry's rule plus the `machine.*`
/// family, which is all hardware.
pub fn acts_on_machine(id: &str) -> bool {
    commands::hardware(id) || id.starts_with("machine.") || id.starts_with("gweike.") || id.starts_with("mcc.")
}

/// Whether the safety gate treats the command as hardware: it acts on a machine, or it changes
/// the machine's setup.
pub fn is_hardware(id: &str) -> bool {
    acts_on_machine(id) || DEVICE_SETUP.contains(&id)
}

/// The hardware class of a hardware command; `None` for an ordinary command.
pub fn hardware_class(id: &str) -> Option<HardwareClass> {
    if !is_hardware(id) {
        return None;
    }
    Some(if STOP.contains(&id) {
        HardwareClass::Stop
    } else if FIRE_OR_JOB.contains(&id) {
        HardwareClass::FireOrJob
    } else if CONNECT.contains(&id) {
        HardwareClass::Connect
    } else if CONFIG.contains(&id) || DEVICE_SETUP.contains(&id) {
        HardwareClass::Config
    } else if MOTION.contains(&id) || id.starts_with("arrange.laser_") {
        HardwareClass::Motion
    } else {
        // Unknown hardware: the strictest class that is not Stop.
        HardwareClass::FireOrJob
    })
}

/// Commands that write a file the user can see (save, export, print to PDF, support data).
const WRITES_FILES: &[&str] = &[
    "device.export_library",
    "file.save",
    "file.save_as",
    "file.save_lbrn2",
    "file.save_gcode",
    "file.export",
    "file.export_gweike_dxf",
    "file.export_gweike_nc",
    "file.print",
    "file.print_colour",
    "file.save_bitmap",
    "file.save_capture",
    "file.prefs_export",
    "help.support_data",
    "project.save_as",
    "project.export",
    "project.print_pdf",
    "project.prefs_export",
    "job.save_gcode",
    "job.log_show", // makes the job log's folder, which is a setting
    "job.preview_save_image",
    "job.preview_save_from_here",
    "job.material_test_presets",
    "image.save_bitmap",
    "image.preset_export",
    "image.scan_offsets_export",
    "library.save",
    "library.save_as",
    "art.save_as",
    "hotkeys.export",
    "support.write",
    "camera.save_capture",
    "camera.export_settings",
];

/// Ids that look like they save or export but only touch the app's own state (settings folder,
/// presets, the clipboard) or return text. Listed so the audit test can tell "forgotten" from
/// "decided".
#[cfg(test)]
pub const NOT_FILE_WRITES: &[&str] = &[
    "image.preset_save",      // the app's own preset store
    "camera.save_settings",   // camera.json in the settings folder
    "camera.import_settings", // reads
    "camera.head_add_capture",
    "edit.image_remove_backup", // a path inside the design
    "file.prefs_backup",        // loads a backup
    "image.trace_capture",      // reads a picture
    "laser.capture",            // updates the camera overlay
    "laser.print_and_cut",      // opens the print and cut window
    "device.export",            // returns the profile as text
];

pub fn writes_files(id: &str) -> bool {
    WRITES_FILES.contains(&id) || crate::machine_ext::writes_files(id)
}

/// Whether the command talks to a Gweike machine. Exports of Gweike file formats are not that
/// (they are file writes); a Gweike command is a hardware command of the `gweike.` / `mcc.`
/// families, or any hardware command while a Gweike device is the active one.
pub fn is_gweike(id: &str, gweike_active: bool) -> bool {
    id.starts_with("gweike.") || id.starts_with("mcc.") || (is_hardware(id) && gweike_active && hardware_class(id) != Some(HardwareClass::Stop))
}

/// Whether the active device is a Gweike machine. The device kinds are matched by name so a
/// driver added later is covered without touching this file.
pub fn gweike_active(app: &DarkBurn) -> bool {
    let kind = format!("{:?}", app.machine.active().kind).to_lowercase();
    kind.contains("gweike") || kind.contains("mcc")
}

// ----- the agent-friendly forms of commands that otherwise open a file dialog -----------------------

/// A command whose registry form opens a native dialog, given an `arguments` form for agents.
pub struct Redirect {
    pub id: &'static str,
    /// The argument-taking command that does the work.
    pub target: &'static str,
    /// A fixed argument added to the call (`format` for the export variants).
    pub fixed: Option<(&'static str, &'static str)>,
}

pub const REDIRECTS: &[Redirect] = &[
    Redirect { id: "file.save_as", target: "project.save_as", fixed: None },
    Redirect { id: "file.export", target: "project.export", fixed: None },
    Redirect { id: "file.save_lbrn2", target: "project.export", fixed: Some(("format", "lbrn2")) },
    Redirect { id: "file.export_gweike_dxf", target: "project.export", fixed: Some(("format", "gweike_dxf")) },
    Redirect { id: "file.export_gweike_nc", target: "project.export", fixed: Some(("format", "gweike_nc")) },
    Redirect { id: "file.save_gcode", target: "job.save_gcode", fixed: None },
    Redirect { id: "file.print", target: "project.print_pdf", fixed: Some(("black_only", "true")) },
    Redirect { id: "file.print_colour", target: "project.print_pdf", fixed: Some(("black_only", "false")) },
    Redirect { id: "file.prefs_export", target: "project.prefs_export", fixed: None },
    Redirect { id: "file.prefs_import", target: "project.prefs_import", fixed: None },
    Redirect { id: "file.save_bitmap", target: "image.save_bitmap", fixed: None },
    Redirect { id: "tools.trace_files", target: "image.trace_files", fixed: None },
    Redirect { id: "help.support_data", target: "support.write", fixed: None },
];

/// Commands that stay out of reach of agents, with the reason.
pub fn refused(id: &str) -> Option<&'static str> {
    match id {
        "file.new_window" => Some("it starts a second DarkBurn window, which an agent cannot drive; use file.new in this window"),
        _ => None,
    }
}

const NEW_PARAMS: &str = r#"{"type":"object","properties":{"discard_changes":{"type":"boolean","description":"throw away unsaved changes (default false: the command fails when the design has unsaved changes)"}},"additionalProperties":false}"#;
const OPEN_PARAMS: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"project file (.dbrn or another format DarkBurn opens)"},"discard_changes":{"type":"boolean","description":"throw away unsaved changes (default false)"}},"required":["path"],"additionalProperties":false}"#;
const IMPORT_PARAMS: &str = r#"{"type":"object","properties":{"path":{"type":"string"},"paths":{"type":"array","items":{"type":"string"}},"x":{"type":"number","description":"where to put the import, mm"},"y":{"type":"number"},"pdf_page":{"description":"a page number, or \"all\""}},"anyOf":[{"required":["path"]},{"required":["paths"]}],"additionalProperties":false}"#;
const COUNT_PARAMS: &str = r#"{"type":"object","properties":{"count":{"type":"integer","minimum":1,"maximum":10000,"description":"how many steps (default 1)"}},"additionalProperties":false}"#;
const SETTINGS_SET: &str = r#"{"type":"object","properties":{"key":{"type":"string","description":"a key from list_settings, e.g. opts.snap_grid, theme, page_width"},"value":{"description":"the new value, of the setting's type"}},"required":["key","value"],"additionalProperties":false}"#;
const REPLACE_PARAMS: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"the picture file"},"fit":{"type":"boolean"}},"required":["path"],"additionalProperties":false}"#;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// `id` as agents call it, with the schema and documentation of the agent form, for the commands
/// that differ from the registry form. `None`: use the registry form.
fn agent_form(id: &str) -> Option<(Value, &'static str)> {
    if let Some(r) = REDIRECTS.iter().find(|r| r.id == id) {
        let mut schema = commands::describe(r.target).map(|i| i.args_schema).unwrap_or(Value::Null);
        if let (Some((k, _)), Some(props)) = (r.fixed, schema.get_mut("properties").and_then(Value::as_object_mut)) {
            props.remove(k);
        }
        let doc = match r.fixed {
            Some(_) => "Takes a file path (no dialog opens for an agent) and writes that format.",
            None => "Takes a file path instead of opening a file dialog (agent form).",
        };
        return Some((schema, doc));
    }
    Some(match id {
        "file.new" => (parse(NEW_PARAMS), "Starts an empty design. Fails when the design has unsaved changes unless discard_changes is true."),
        "file.open" => (parse(OPEN_PARAMS), "Opens a project by path (agent form of the Open dialog)."),
        "file.save" => (
            parse(r#"{"type":"object","properties":{},"additionalProperties":false}"#),
            "Saves to the project's file. A design that has no file yet needs file.save_as with a path.",
        ),
        "file.import" => (parse(IMPORT_PARAMS), "Imports files by path (agent form of the Import dialog)."),
        "edit.undo" => (parse(COUNT_PARAMS), "Undo one or `count` steps."),
        "edit.redo" => (parse(COUNT_PARAMS), "Redo one or `count` steps."),
        "edit.image_replace" | "edit.image_replace_fit" => (parse(REPLACE_PARAMS), "Replaces the selected image from a file (agent form)."),
        "settings.set" => {
            (parse(SETTINGS_SET), "Changes one setting by its key (see list_settings). The AI-agent switches themselves cannot be changed this way.")
        }
        _ => return None,
    })
}

/// The command `id` as MCP reports it, from the registry entry `info`.
pub fn command_info(app: &DarkBurn, info: &commands::Info, gweike_active: bool) -> CommandInfo {
    let id = info.id.as_str();
    let mut c = CommandInfo::new(id, &info.title, &info.group);
    c.menu_path = info.menu_path.clone();
    c.shortcut = (!info.shortcut.is_empty()).then(|| info.shortcut.clone());
    c.checklist_ids = info.checklist.clone();
    c.params_schema = if info.args_schema.as_object().is_some_and(|o| o.is_empty()) { Value::Null } else { info.args_schema.clone() };
    let hardware = is_hardware(id);
    c.hardware = hardware;
    c.hardware_class = hardware_class(id);
    c.gweike = is_gweike(id, gweike_active);
    c.writes_files = writes_files(id);
    c.enabled = info.implemented && app.can_run(id) && refused(id).is_none();
    let mut doc = format!("{}. Menu: {}.", info.title, if info.menu_path.is_empty() { "none".to_string() } else { info.menu_path.join(" > ") });
    if let Some((schema, extra)) = agent_form(id) {
        c.params_schema = schema;
        doc.push(' ');
        doc.push_str(extra);
    }
    if !info.implemented && !c.enabled {
        doc.push_str(" Not implemented in this build.");
    }
    if let Some(why) = refused(id) {
        doc.push_str(&format!(" Not available to agents: {why}."));
    }
    if let Some(class) = c.hardware_class {
        doc.push_str(&format!(" Hardware ({class:?}): denied to agents until the user allows AI control of machines."));
        if class == HardwareClass::FireOrJob {
            doc.push_str(" Also needs the user's confirmation.");
        }
        if class == HardwareClass::Stop {
            doc.push_str(" Always allowed.");
        }
    }
    if c.writes_files {
        doc.push_str(" Writes a file: denied when the user turned file writes off.");
    }
    c.doc = doc;
    c
}

const CUT_SETTINGS: &str = r#"{"type":"object","properties":{"layer":{"type":"integer","minimum":0,"maximum":31},"sublayer":{"type":"integer","minimum":0,"description":"0 is the layer's main settings (default), 1 and up its sub-layers"},"settings":{"type":"object","description":"cut setting names and new values, as inspect_design layers.N shows them (mode as line, fill, fill_line, offset_fill, image)"}},"required":["layer","settings"],"additionalProperties":false}"#;

/// Commands that exist only for MCP (no menu item).
pub fn extra_commands() -> Vec<CommandInfo> {
    let mut c = CommandInfo::new("settings.set", "Change a Setting by Key", "settings");
    c.params_schema = parse(SETTINGS_SET);
    c.doc = "Changes one preference by its dotted key from list_settings (for example opts.snap_grid or page_width). The type must match. The AI-agent switches (mcp.*) cannot be changed through MCP.".into();
    let mut l = CommandInfo::new("layer.set_cut_settings", "Change Any Cut Settings of a Layer", "layer");
    l.params_schema = parse(CUT_SETTINGS);
    l.checklist_ids = vec!["CUT-01".into()];
    l.doc = "Changes any of a layer's cut settings (the fields of the Cut Settings editor: kerf, overscan, tabs, ramps, lead-in, Z offset ...) in one undo step. Names and values are those inspect_design layers.N reports; unknown names and wrong types fail.".into();
    vec![c, l]
}
