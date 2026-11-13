//! Tests of the MCP backend: coverage, classification, the safety gate in front of the real
//! command path, and the whole stack over the real control channel.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use darkburn_mcp::control::{Connect, ControlConfig, ControlServer, RemoteBackend};
use darkburn_mcp::server::Server;
use darkburn_mcp::tools::{DarkburnTools, INFO};
use darkburn_mcp::{
    Backend, BatchStep, CallContext, CommandError, CommandInfo, CommandPage, ErrorCode, EventPage, Gate, HardwareClass, ListQuery, McpPolicy,
    PendingState, PolicyHandle, Region, assert_full_coverage,
};
use serde_json::{Value, json};

use crate::commands;
use crate::mcp_backend::{McpBackend, SharedApp};
use crate::mcp_catalog;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

fn shared() -> SharedApp {
    Rc::new(RefCell::new(DarkBurn::new(Prefs::default())))
}

fn backend() -> McpBackend {
    McpBackend::new(shared(), egui::Context::default(), true)
}

fn exec(b: &mut dyn Backend, id: &str, args: Value) -> Result<Value, CommandError> {
    b.execute(id, args)
}

/// Wait for a long operation the way the tool layer does.
fn finish(b: &mut dyn Backend, r: Result<Value, CommandError>) -> Result<Value, CommandError> {
    match r {
        Err(e) if e.code == ErrorCode::StillRunning => {
            let id = e.pending_id.expect("a pending id");
            for _ in 0..4000 {
                match b.pending(&id).unwrap() {
                    PendingState::Running { .. } => std::thread::sleep(Duration::from_millis(5)),
                    PendingState::Done { result, .. } => return result,
                    PendingState::Unknown => panic!("the operation vanished"),
                }
            }
            panic!("the operation never finished");
        }
        other => other,
    }
}

// ----- coverage and the registry --------------------------------------------------------------------

#[test]
fn every_ui_action_has_a_command() {
    let mut b = backend();
    assert_full_coverage(&mut b, &commands::ui_action_ids());
}

#[test]
fn every_registered_command_has_a_valid_schema() {
    for c in commands::registry() {
        if c.args.is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(c.args).unwrap_or_else(|e| panic!("{}: the argument schema is not JSON: {e}", c.id));
        assert_eq!(v["type"], "object", "{}: the schema root must be an object", c.id);
    }
    let mut b = backend();
    let all = b.list_commands(&ListQuery::default()).unwrap().commands;
    assert!(all.len() > 300, "the whole registry is listed: {}", all.len());
    for c in &all {
        if !c.params_schema.is_null() {
            darkburn_mcp::util::check_schema_document(&c.params_schema).unwrap_or_else(|e| panic!("{}: {e}", c.id));
        }
        assert!(!c.doc.is_empty(), "{} has a description", c.id);
    }
}

#[test]
fn hardware_classification_is_decided_for_every_command() {
    // (id, class). Everything that moves axes, fires, sends, connects or writes controller
    // settings is hardware; stop and pause are class Stop.
    let table: &[(&str, HardwareClass)] = &[
        ("machine.connect", HardwareClass::Connect),
        ("machine.disconnect", HardwareClass::Connect),
        ("machine.status", HardwareClass::Connect),
        ("machine.read_settings", HardwareClass::Connect),
        ("machine.jog", HardwareClass::Motion),
        ("machine.jog_start", HardwareClass::Motion),
        ("machine.move_to", HardwareClass::Motion),
        ("machine.home", HardwareClass::Motion),
        ("machine.frame", HardwareClass::Motion),
        ("machine.go_to_origin", HardwareClass::Motion),
        ("machine.set_origin", HardwareClass::Motion),
        ("machine.write_setting", HardwareClass::Config),
        ("machine.dsp_write", HardwareClass::Config),
        ("machine.file_upload", HardwareClass::Config),
        ("machine.file_delete", HardwareClass::Config),
        ("machine.overrides", HardwareClass::Config),
        ("edit.device_settings", HardwareClass::Config),
        ("edit.machine_settings", HardwareClass::Config),
        ("machine.start", HardwareClass::FireOrJob),
        ("machine.resume", HardwareClass::FireOrJob),
        ("machine.fire", HardwareClass::FireOrJob),
        ("machine.console", HardwareClass::FireOrJob),
        ("machine.macro", HardwareClass::FireOrJob),
        ("machine.file_run", HardwareClass::FireOrJob),
        ("job.start", HardwareClass::FireOrJob),
        ("job.send", HardwareClass::FireOrJob),
        ("job.run_gcode", HardwareClass::FireOrJob),
        ("job.preview_run_from_here", HardwareClass::FireOrJob),
        ("job.frame", HardwareClass::Motion),
        ("job.home", HardwareClass::Motion),
        ("job.go_origin", HardwareClass::Motion),
        ("tools.position_laser", HardwareClass::Motion),
        ("arrange.to_laser", HardwareClass::Motion),
        ("arrange.laser_centre", HardwareClass::Motion),
        ("laser.controller_info", HardwareClass::Connect),
        ("laser.devices", HardwareClass::Connect),
        ("laser.repeat_marking", HardwareClass::FireOrJob),
        ("machine.stop", HardwareClass::Stop),
        ("machine.pause", HardwareClass::Stop),
        ("machine.fire_off", HardwareClass::Stop),
        ("machine.jog_stop", HardwareClass::Stop),
        ("job.stop", HardwareClass::Stop),
        ("job.pause", HardwareClass::Stop),
    ];
    let mut b = backend();
    for (id, class) in table {
        let c = b.describe_command(id).unwrap().unwrap_or_else(|| panic!("{id} is registered"));
        assert!(c.hardware, "{id} must be hardware");
        assert_eq!(c.hardware_class, Some(*class), "{id}");
    }
    // The families: every machine.* id, every laser.* that is not a document tool, is hardware.
    let all = b.list_commands(&ListQuery::default()).unwrap().commands;
    for c in &all {
        if c.id.starts_with("machine.") || c.id.starts_with("arrange.laser_") {
            assert!(c.hardware, "{} must be hardware", c.id);
        }
        assert_eq!(c.hardware, c.hardware_class.is_some(), "{}: hardware and class go together", c.id);
        if c.hardware_class == Some(HardwareClass::Stop) {
            assert!(
                c.id.contains("stop") || c.id.contains("pause") || c.id.ends_with("fire_off"),
                "{}: only stop-like commands may be class Stop",
                c.id
            );
        }
        // Document edits are never hardware, so an agent can always work on the design.
        if c.id.starts_with("shape.") || c.id.starts_with("layer.") || c.id.starts_with("selection.") || c.id.starts_with("edit.select") {
            assert!(!c.hardware, "{} only edits the document", c.id);
        }
    }
    // Starting a job is hardware even though the preview and G-code file are not.
    for id in ["file.save_gcode", "job.generate", "job.save_gcode", "window.preview", "laser.material_test"] {
        let c = b.describe_command(id).unwrap().unwrap();
        assert!(!c.hardware, "{id} builds a job without touching a machine");
    }
}

#[test]
fn writes_files_is_decided_for_everything_that_looks_like_saving() {
    let mut b = backend();
    let all = b.list_commands(&ListQuery::default()).unwrap().commands;
    for c in &all {
        let looks = ["save", "export", "print", "backup", "capture", "dump"].iter().any(|w| c.id.contains(w));
        if looks && !c.hardware {
            assert!(
                c.writes_files || mcp_catalog::NOT_FILE_WRITES.contains(&c.id.as_str()),
                "{} looks like it writes a file: list it in WRITES_FILES or NOT_FILE_WRITES",
                c.id
            );
        }
    }
    for id in ["file.save", "file.export", "file.export_gweike_dxf", "file.export_gweike_nc", "job.save_gcode", "project.export", "support.write"] {
        let c = b.describe_command(id).unwrap().unwrap();
        assert!(c.writes_files, "{id}");
        assert!(!c.gweike, "{id}: a Gweike file export does not drive a Gweike machine");
        assert!(!c.hardware, "{id}");
    }
    assert!(!mcp_catalog::is_gweike("file.export_gweike_dxf", false));
    assert!(mcp_catalog::is_gweike("machine.jog", true), "hardware on a Gweike device is Gweike");
    assert!(!mcp_catalog::is_gweike("machine.stop", true), "stop is always allowed");
    assert!(mcp_catalog::is_gweike("gweike.anything", false));
    assert!(mcp_catalog::is_gweike("mcc.read_registers", false));
}

// ----- the gate in front of the real commands ---------------------------------------------------------

/// Counts what reaches the backend's `execute`.
struct Spy {
    inner: McpBackend,
    executed: Rc<RefCell<Vec<String>>>,
}

impl Backend for Spy {
    fn list_commands(&mut self, q: &ListQuery) -> Result<CommandPage, CommandError> {
        self.inner.list_commands(q)
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        self.inner.describe_command(id)
    }
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        self.executed.borrow_mut().push(id.to_string());
        self.inner.execute(id, args)
    }
    fn execute_ctx(&mut self, id: &str, args: Value, _ctx: &CallContext) -> Result<Value, CommandError> {
        self.execute(id, args)
    }
    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> {
        self.inner.inspect(path, depth)
    }
    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        self.inner.events(since)
    }
    fn confirmation_scope(&mut self) -> String {
        self.inner.confirmation_scope()
    }
    fn request_confirmation(&mut self, id: &str, c: &str, s: &str) -> Result<(), CommandError> {
        self.inner.request_confirmation(id, c, s)
    }
}

fn spy_gate(policy: McpPolicy) -> (Gate<Spy>, Rc<RefCell<Vec<String>>>, SharedApp) {
    let app = shared();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let spy = Spy { inner: McpBackend::new(app.clone(), egui::Context::default(), true), executed: seen.clone() };
    (Gate::new(spy, PolicyHandle::new(policy)), seen, app)
}

#[test]
fn a_denied_hardware_command_never_reaches_the_command_registry() {
    let (mut gate, seen, app) = spy_gate(McpPolicy::default());
    let all = gate.list_commands(&ListQuery::default()).unwrap().commands;
    let mut tried = 0;
    for c in all.iter().filter(|c| c.hardware && c.hardware_class != Some(HardwareClass::Stop)) {
        // With empty arguments, and with the arguments the command wants.
        for args in [json!({}), sample_args(&c.id)] {
            let r = gate.execute(&c.id, args);
            let e = r.expect_err(&format!("{} must not run by default", c.id));
            assert!(
                matches!(e.code, ErrorCode::Denied | ErrorCode::InvalidParams | ErrorCode::Disabled),
                "{}: expected a refusal, got {:?} {}",
                c.id,
                e.code,
                e.message
            );
            tried += 1;
        }
    }
    assert!(tried > 60, "the sweep covered the hardware commands: {tried}");
    assert!(seen.borrow().is_empty(), "nothing reached the backend: {:?}", seen.borrow());
    for (id, args) in [
        ("machine.jog", json!({"dx": 1.0})),
        ("job.start", json!({})),
        ("machine.fire", json!({"percent": 1, "secs": 0.1})),
        ("machine.connect", json!({})),
    ] {
        let e = gate.execute(id, args).unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied, "{id}: {}", e.message);
        assert!(e.message.contains("ask the user"), "{id}: the refusal tells the agent to ask: {}", e.message);
    }
    assert!(seen.borrow().is_empty());
    // Stop is always allowed, whatever the policy says: it reaches the backend.
    let _ = gate.execute("machine.stop", json!({}));
    assert_eq!(seen.borrow().as_slice(), ["machine.stop"]);
    drop(app);
}

fn sample_args(id: &str) -> Value {
    match id {
        "machine.jog" => json!({"dx": 1.0}),
        "machine.move_to" => json!({"x": 1.0, "y": 1.0}),
        "machine.console" => json!({"line": "G0 X1"}),
        "machine.macro" => json!({"text": "G0 X1"}),
        "machine.write_setting" => json!({"id": 1, "value": "1"}),
        "machine.frame" => json!({"x": 0, "y": 0, "width": 1, "height": 1}),
        "machine.file_run" | "machine.file_calc_time" => json!({"index": 0}),
        "machine.file_delete" => json!({"indices": [0]}),
        "machine.file_upload" => json!({"path": "x.rd"}),
        "machine.show_all" => json!({"on": true}),
        "machine.dsp_read" => json!({"what": "laser2"}),
        "machine.dsp_write" => json!({"what": "laser2", "values": {}}),
        "machine.file_overwrite" => json!({"choice": "cancel"}),
        "job.preview_run_from_here" | "job.preview_send_from_here" => json!({"op_index": 0}),
        _ => json!({}),
    }
}

#[test]
fn with_machine_control_on_a_job_start_needs_the_users_click() {
    let policy = McpPolicy { allow_hardware: true, ..McpPolicy::default() };
    let (mut gate, seen, _app) = spy_gate(policy);
    let e = gate.execute("job.start", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{}", e.message);
    let pid = e.pending_id.unwrap();
    assert!(seen.borrow().is_empty());
    // The dialog is open and shows what the agent wants.
    let open = gate.open_confirmations();
    assert_eq!(open.len(), 1);
    assert!(open[0].2.contains("job.start"), "{:?}", open[0]);
    assert!(matches!(gate.pending(&pid).unwrap(), PendingState::Running { .. }));
    // Cancel: no token, nothing runs.
    gate.resolve_confirmation(&pid, false).unwrap();
    assert!(matches!(gate.pending(&pid).unwrap(), PendingState::Done { result: Err(_), .. }));
    // Ask again, confirm, and the token works once for these exact arguments.
    let pid = gate.execute("job.start", json!({})).unwrap_err().pending_id.unwrap();
    gate.resolve_confirmation(&pid, true).unwrap();
    let PendingState::Done { result: Ok(v), .. } = gate.pending(&pid).unwrap() else { panic!("confirmed") };
    let token = v["confirmation_token"].as_str().unwrap().to_string();
    let ctx = CallContext { tool: "execute".into(), confirmation_token: Some(token.clone()) };
    let _ = gate.execute_ctx("job.start", json!({}), &ctx);
    assert_eq!(seen.borrow().as_slice(), ["job.start"], "it ran once the user confirmed");
    let again = gate.execute_ctx("job.start", json!({}), &ctx).unwrap_err();
    assert_eq!(again.code, ErrorCode::ConfirmationInvalid, "a token works once");
    assert_eq!(seen.borrow().len(), 1);
    // Jogging is not a job start: with control on it needs no click.
    let _ = gate.execute("machine.jog", json!({"dx": 1.0}));
    assert_eq!(seen.borrow().len(), 2);
}

#[test]
fn the_policy_cannot_be_changed_by_an_agent() {
    let mut b = backend();
    // Not through settings.set ...
    let e = exec(&mut b, "settings.set", json!({"key": "mcp.allow_hardware", "value": true})).unwrap_err();
    assert!(e.message.contains("only be changed by the user"), "{}", e.message);
    let e = exec(&mut b, "settings.set", json!({"key": "mcp", "value": {}})).unwrap_err();
    assert!(e.message.contains("only be changed by the user"), "{}", e.message);
    // ... not through a preferences file ...
    let dir = std::env::temp_dir().join(format!("darkburn-mcp-pol-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut evil = Prefs::default();
    evil.mcp.enabled = true;
    evil.mcp.allow_hardware = true;
    evil.mcp.allow_gweike = true;
    evil.mcp.require_confirmation = false;
    // (Not the work area: that stays the active device's when preferences are imported.)
    evil.invert_wheel = true;
    let file = dir.join("evil.json");
    std::fs::write(&file, evil.to_json()).unwrap();
    exec(&mut b, "project.prefs_import", json!({"path": file.to_string_lossy()})).unwrap();
    let app = b.app_for_test();
    assert!(app.borrow().prefs.invert_wheel, "the other preferences did import");
    assert_eq!(app.borrow().prefs.mcp, Prefs::default().mcp, "the AI-agent switches did not");
    // ... and the settings listing does not show them.
    let list = b.inspect("settings", 0).unwrap();
    assert!(list.as_array().unwrap().iter().all(|s| !s["key"].as_str().unwrap().starts_with("mcp")));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- the backend ------------------------------------------------------------------------------------

#[test]
fn an_agent_builds_a_design_and_reads_it_back() {
    let mut b = backend();
    let r = exec(&mut b, "shape.add_rect", json!({"x": 10, "y": 10, "width": 40, "height": 20, "layer": 2})).unwrap();
    let id = r["id"].as_u64().unwrap();
    exec(&mut b, "shape.add_ellipse", json!({"x": 100, "y": 10, "width": 30, "height": 30})).unwrap();
    let d = b.inspect("design", 0).unwrap();
    assert_eq!(d["shape_count"], 2);
    let s = &d["shapes"][0];
    assert_eq!(s["id"], format!("s{id}"));
    assert_eq!(s["kind"], "Rectangle");
    assert_eq!(s["layer"], 2);
    assert_eq!(s["bbox"], json!([10.0, 10.0, 50.0, 30.0]));
    // Stable id addressing, layers by index or name, with every cut setting.
    assert_eq!(b.inspect(&format!("design.shapes.s{id}"), 0).unwrap()["width"], 40.0);
    exec(&mut b, "layer.set_mode", json!({"layer": 2, "mode": "fill"})).unwrap();
    exec(&mut b, "layer.set_param", json!({"layer": 2, "field": "speed", "value": 250})).unwrap();
    let l = b.inspect("design.layers.2", 0).unwrap();
    assert_eq!(l["mode"], "fill");
    assert_eq!(l["speed"], 250.0);
    assert!(l["settings"][0]["interval"].is_number(), "every cut setting is there: {l}");
    let by_name = b.inspect(&format!("design.layers.{}", l["name"].as_str().unwrap()), 0).unwrap();
    assert_eq!(by_name["index"], 2);
    // Undo with a count, and the stack labels.
    let u = b.inspect("undo", 0).unwrap();
    assert_eq!(u["undo"][0], "Layer setting");
    exec(&mut b, "edit.undo", json!({"count": 2})).unwrap();
    assert_eq!(b.inspect("design.layers.2", 0).unwrap()["mode"], "line");
    assert_eq!(b.inspect("undo", 0).unwrap()["redo"][0], "Layer mode");
    assert!(b.inspect("design.shapes.s999", 0).is_err());
    // Measure.
    let m = b.measure(json!({"target": "all"})).unwrap();
    assert_eq!(m["shapes"], 2);
    assert!((m["per_shape"][0]["length_mm"].as_f64().unwrap() - 120.0).abs() < 0.01);
    assert!((m["per_shape"][0]["area_mm2"].as_f64().unwrap() - 800.0).abs() < 0.01);
}

#[test]
fn any_cut_setting_can_be_changed_and_read_back() {
    let mut b = backend();
    exec(&mut b, "shape.add_rect", json!({"x": 0, "y": 0, "width": 10, "height": 10, "layer": 3})).unwrap();
    exec(
        &mut b,
        "layer.set_cut_settings",
        json!({"layer": 3, "settings": {"kerf": 0.15, "mode": "fill_line", "overscan": 2.5, "cross_hatch": true}}),
    )
    .unwrap();
    let l = b.inspect("design.layers.3", 0).unwrap();
    assert_eq!(l["mode"], "fill_line");
    let s = &l["settings"][0];
    assert_eq!((s["kerf"].as_f64(), s["overscan"].as_f64(), s["cross_hatch"].as_bool()), (Some(0.15), Some(2.5), Some(true)));
    assert_eq!(b.inspect("undo", 0).unwrap()["undo"][0], "Cut settings");
    assert!(exec(&mut b, "layer.set_cut_settings", json!({"layer": 3, "settings": {"no_such": 1}})).is_err());
    assert!(exec(&mut b, "layer.set_cut_settings", json!({"layer": 3, "settings": {"kerf": "wide"}})).is_err());
    assert!(exec(&mut b, "layer.set_cut_settings", json!({"layer": 3, "sublayer": 4, "settings": {"kerf": 1}})).is_err());
    exec(&mut b, "edit.undo", json!({})).unwrap();
    assert_eq!(b.inspect("design.layers.3", 0).unwrap()["mode"], "line");
}

#[test]
fn the_agent_forms_of_the_dialog_commands_work_without_a_window() {
    let dir = std::env::temp_dir().join(format!("darkburn-mcp-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut b = backend();
    exec(&mut b, "shape.add_polygon", json!({"cx": 50, "cy": 50, "sides": 5, "radius": 10})).unwrap();
    let svg = dir.join("out.svg");
    exec(&mut b, "file.export", json!({"path": svg.to_string_lossy()})).unwrap();
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<svg"));
    let lb = dir.join("out.lbrn2");
    exec(&mut b, "file.save_lbrn2", json!({"path": lb.to_string_lossy()})).unwrap();
    assert!(std::fs::read_to_string(&lb).unwrap().contains("LightBurnProject"));
    // Unsaved changes are not thrown away unless asked.
    let e = exec(&mut b, "file.new", json!({})).unwrap_err();
    assert!(e.message.contains("unsaved changes"), "{}", e.message);
    assert_eq!(b.inspect("design", 0).unwrap()["shape_count"], 1);
    let e = exec(&mut b, "file.save", json!({})).unwrap_err();
    assert!(e.message.contains("file.save_as"), "{}", e.message);
    let proj = dir.join("a.dbrn");
    exec(&mut b, "file.save_as", json!({"path": proj.to_string_lossy()})).unwrap();
    exec(&mut b, "file.save", json!({})).unwrap();
    exec(&mut b, "file.new", json!({})).unwrap();
    assert_eq!(b.inspect("design", 0).unwrap()["shape_count"], 0);
    exec(&mut b, "file.open", json!({"path": proj.to_string_lossy()})).unwrap();
    assert_eq!(b.inspect("design", 0).unwrap()["shape_count"], 1);
    exec(&mut b, "file.import", json!({"path": svg.to_string_lossy(), "x": 0, "y": 0})).unwrap();
    assert!(b.inspect("design", 0).unwrap()["shape_count"].as_u64().unwrap() >= 2);
    // Commands that would open a window of their own are refused with a reason.
    let e = exec(&mut b, "file.new_window", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotSupported);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_can_be_listed_read_and_changed() {
    let mut b = backend();
    let list = b.inspect("settings", 0).unwrap();
    let keys: Vec<&str> = list.as_array().unwrap().iter().map(|s| s["key"].as_str().unwrap()).collect();
    assert!(keys.contains(&"page_width") && keys.contains(&"opts.snap_grid"), "{keys:?}");
    assert_eq!(b.inspect("settings.page_width", 0).unwrap()["value"], 400.0);
    exec(&mut b, "settings.set", json!({"key": "opts.snap_grid", "value": false})).unwrap();
    assert_eq!(b.inspect("settings.opts.snap_grid", 0).unwrap()["value"], false);
    exec(&mut b, "settings.set", json!({"key": "page_width", "value": 300})).unwrap();
    assert_eq!(b.inspect("settings.page_width", 0).unwrap()["value"], 300.0);
    assert!(exec(&mut b, "settings.set", json!({"key": "page_width", "value": "wide"})).is_err());
    assert!(exec(&mut b, "settings.set", json!({"key": "no.such", "value": 1})).is_err());
}

#[test]
fn ui_state_lists_dialogs_panels_tools_and_shortcuts() {
    let mut b = backend();
    let ui = b.inspect("ui", 0).unwrap();
    assert_eq!(ui["headless"], true);
    assert!(ui["panels"].as_array().unwrap().iter().any(|p| p["id"] == "cuts"));
    assert!(ui["shortcuts"].as_array().unwrap().iter().any(|s| s["command"] == "file.save" && s["key"] == "Ctrl+S"));
    assert_eq!(ui["tool"], "select");
    exec(&mut b, "tools.rect", json!({})).unwrap();
    exec(&mut b, "edit.settings", json!({})).unwrap();
    let ui = b.inspect("ui", 0).unwrap();
    assert_eq!(ui["tool"], "rect");
    assert!(ui["dialogs"].as_array().unwrap().iter().any(|d| d == "settings"));
    let ev = b.events(0).unwrap();
    assert!(ev.events.iter().any(|e| e.kind == "dialog" && e.outcome == "opened" && e.message == "settings"), "{:?}", ev.events);
    assert_eq!(b.inspect("ui.tool", 0).unwrap(), "rect");
    // Machines, library, undo, job.
    assert!(b.inspect("machine.devices", 0).unwrap().as_array().unwrap().iter().any(|d| d["simulator"] == true));
    assert!(b.inspect("machine.status", 0).unwrap()["state"].is_string());
    assert!(b.inspect("library", 0).unwrap()["cut_library"]["entries"].is_array());
    assert!(b.inspect("camera", 0).unwrap().is_object());
    assert!(b.inspect("print_and_cut", 0).unwrap().is_object());
    assert!(b.inspect("rotary", 0).unwrap().is_object());
    assert!(b.inspect("job", 0).unwrap()["settings"].is_object());
    assert!(b.inspect("nonsense", 0).is_err());
    // Errors are events too.
    let _ = exec(&mut b, "edit.undo", json!({}));
    assert!(b.events(0).unwrap().events.iter().any(|e| e.kind == "error"));
}

#[test]
fn a_job_preview_is_built_off_the_ui_thread_and_never_sent() {
    let mut b = backend();
    exec(&mut b, "shape.add_rect", json!({"x": 10, "y": 10, "width": 40, "height": 20, "layer": 0})).unwrap();
    exec(&mut b, "layer.set_param", json!({"layer": 0, "field": "speed", "value": 50})).unwrap();
    let r = b.preview_job(json!({"format": "gcode"}));
    let e = r.as_ref().unwrap_err();
    assert_eq!(e.code, ErrorCode::StillRunning, "a job build is a long operation");
    assert!(b.running().unwrap().len() <= 1);
    let v = finish(&mut b, r).unwrap();
    assert!(v["ops"].as_u64().unwrap() > 0, "{v}");
    assert!(v["seconds"].as_f64().unwrap() > 0.0);
    assert!(v["cut_mm"].as_f64().unwrap() > 100.0, "{v}");
    assert_eq!(v["sent_to_machine"], false);
    assert!(v["gcode"].as_str().unwrap().contains("G1") || v["gcode"].as_str().unwrap().contains("G0"), "{}", v["gcode"]);
    assert!(v["op_counts"].is_object() && v["layers_detail"][0]["speed"] == 50.0, "{v}");
    assert!(b.running().unwrap().is_empty());
    // The machine was not touched.
    let status = b.inspect("machine.status", 0).unwrap();
    assert!(status["job"].is_null(), "{status}");
    // job.generate and measuring the job go the same way; G-code is cut to a limit on request.
    let r = exec(&mut b, "job.generate", json!({}));
    let g = finish(&mut b, r).unwrap();
    assert!(g["ops"].as_u64().unwrap() > 0);
    let r = b.preview_job(json!({"format": "gcode", "max_lines": 3}));
    let small = finish(&mut b, r).unwrap();
    assert_eq!(small["gcode_truncated"], true);
    let r = b.measure(json!({"target": "design", "what": ["time"]}));
    let m = finish(&mut b, r).unwrap();
    assert!(m["job"]["seconds"].as_f64().unwrap() > 0.0 && m["shapes"] == 1, "{m}");
}

/// The G-code an agent is shown is what the selected device would be sent, not GRBL's.
#[test]
fn a_job_preview_shows_the_g_code_of_the_selected_device() {
    let app = shared();
    let mut b = McpBackend::new(app.clone(), egui::Context::default(), true);
    {
        let mut a = app.borrow_mut();
        let id = crate::api::execute_headless(&mut a, "device.add", &json!({ "type": "Marlin" })).unwrap()["id"].as_u64().unwrap();
        crate::api::execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
    }
    exec(&mut b, "shape.add_rect", json!({"x": 10, "y": 10, "width": 40, "height": 20, "layer": 0})).unwrap();
    let r = b.preview_job(json!({"format": "gcode"}));
    let v = finish(&mut b, r).unwrap();
    let shown = v["gcode"].as_str().unwrap().to_owned();
    let a = app.borrow();
    let job = a.job.last_job.clone().expect("the job that was built");
    let sent = crate::gcode_machines::job_text(a.machine.active(), &job, None, &a.job.gcode);
    assert_eq!(shown, sent, "the preview is not the device's G-code");
    assert!(!shown.lines().any(|l| l == "M2" || l.starts_with("M4")), "GRBL words in a Marlin job: {shown}");
}

#[test]
fn a_batch_that_fails_is_rolled_back_with_the_editors_state() {
    let mut b = backend();
    exec(&mut b, "shape.add_rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    let steps = [
        BatchStep { command: "shape.add_ellipse".into(), params: json!({"x": 0, "y": 0, "width": 5, "height": 5}), confirmation_token: None },
        BatchStep { command: "shape.rotate".into(), params: json!({"degrees": 45}), confirmation_token: None },
        BatchStep { command: "layer.set_param".into(), params: json!({"layer": 99, "field": "speed", "value": 1}), confirmation_token: None },
    ];
    let r = b.batch(&steps, true).unwrap();
    assert!(!r.ok && r.rolled_back && r.failed_step == Some(2), "{r:?}");
    let d = b.inspect("design", 0).unwrap();
    assert_eq!(d["shape_count"], 1, "the ellipse is gone again");
    assert_eq!(d["shapes"][0]["bbox"], json!([0.0, 0.0, 10.0, 10.0]));
    let r = b.batch(&steps[..2], true).unwrap();
    assert!(r.ok && !r.rolled_back);
    assert_eq!(b.inspect("design", 0).unwrap()["shape_count"], 2);
}

#[test]
fn the_headless_screenshot_is_an_offscreen_drawing() {
    let mut b = backend();
    exec(&mut b, "shape.add_rect", json!({"x": 10, "y": 10, "width": 100, "height": 60, "layer": 1})).unwrap();
    let png = b.screenshot(None).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert!(img.width() >= 400 && img.height() >= 400);
    let inked = img.pixels().filter(|p| p.0[0] > 150 && p.0[1] < 120).count();
    assert!(inked > 200, "the red layer 1 outline is drawn: {inked}");
    let part = b.screenshot(Some(Region { x: 0.0, y: 0.0, width: 60.0, height: 40.0 })).unwrap();
    assert!(image::load_from_memory(&part).is_ok());
}

#[test]
fn the_guide_names_every_command_group() {
    let guide = darkburn_mcp::tools::AGENT_GUIDE;
    let mut b = backend();
    let all = b.list_commands(&ListQuery::default()).unwrap().commands;
    let mut groups: Vec<&str> = all.iter().map(|c| c.id.split('.').next().unwrap()).collect();
    groups.sort();
    groups.dedup();
    let missing: Vec<&str> = groups.into_iter().filter(|g| !(guide.contains(&format!("`{g}.*`")) || guide.contains(&format!("`{g}.")))).collect();
    assert!(missing.is_empty(), "the agent guide does not mention these command groups: {missing:?}");
    // The commands the worked example uses exist and their parameters validate.
    for (id, args) in [
        ("shape.add_rect", json!({"x": 10, "y": 10, "width": 60, "height": 40, "layer": 1})),
        ("layer.set_mode", json!({"layer": 1, "mode": "fill"})),
        ("layer.set_param", json!({"layer": 1, "field": "interval", "value": 0.1})),
        ("layer.set_cut_settings", json!({"layer": 1, "settings": {"kerf": 0.1, "overscan": 2}})),
    ] {
        let mut g = Gate::new(backend(), PolicyHandle::default());
        let r = g.execute(id, args);
        assert!(r.is_ok(), "{id}: {r:?}");
    }
}

#[test]
fn unclassified_hardware_defaults_to_the_strictest_class() {
    assert_eq!(mcp_catalog::hardware_class("machine.brand_new"), Some(HardwareClass::FireOrJob));
    assert_eq!(mcp_catalog::hardware_class("gweike.anything"), Some(HardwareClass::FireOrJob));
    assert_eq!(mcp_catalog::hardware_class("shape.add_rect"), None);
}

// ----- the whole stack over the control channel -----------------------------------------------------------

const TOKEN: &str = "app-test-token-0123456789abcdef";

type Job = Box<dyn FnOnce(&mut Gate<McpBackend>) + Send>;

/// Stands in for the window: a thread that owns the app, the backend and the gate and pumps the
/// server once per "frame", like `logic()` does.
struct Window {
    port: u16,
    stop: Arc<AtomicBool>,
    jobs: mpsc::Sender<Job>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Window {
    fn start(policy: McpPolicy) -> Window {
        let stop = Arc::new(AtomicBool::new(false));
        let (ptx, prx) = mpsc::channel();
        let (jtx, jrx) = mpsc::channel::<Job>();
        let s2 = stop.clone();
        let join = std::thread::spawn(move || {
            let server = ControlServer::bind(ControlConfig { port: 0, token: Some(TOKEN.into()), ..Default::default() }).expect("bind");
            ptx.send(server.port()).unwrap();
            let mut gate = Gate::new(McpBackend::new(shared(), egui::Context::default(), true), PolicyHandle::new(policy));
            while !s2.load(Ordering::SeqCst) {
                gate.inner().poll();
                server.pump(&mut gate, 64);
                while let Ok(job) = jrx.try_recv() {
                    job(&mut gate);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let port = prx.recv_timeout(Duration::from_secs(5)).unwrap();
        Window { port, stop, jobs: jtx, join: Some(join) }
    }

    fn mcp(&self) -> Server<DarkburnTools> {
        let remote = RemoteBackend::new(Connect::Direct { port: self.port, token: TOKEN.into() });
        Server::new(INFO, DarkburnTools::new(Box::new(remote)).with_poll(Duration::from_millis(2)))
    }

    /// Run `f` on the window thread (where the gate lives) and wait for it.
    fn on_ui<R: Send + 'static>(&self, f: impl FnOnce(&mut Gate<McpBackend>) -> R + Send + 'static) -> R {
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Box::new(move |g| {
                let _ = tx.send(f(g));
            }))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn call(server: &mut Server<DarkburnTools>, tool: &str, args: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}}).to_string();
    let reply: Value = serde_json::from_str(&server.handle_line(&line).expect("a reply")).unwrap();
    assert!(reply.get("error").is_none(), "{tool}: {reply}");
    reply["result"].clone()
}

fn text(r: &Value) -> String {
    r["content"][0]["text"].as_str().unwrap_or("").to_string()
}

fn json_of(r: &Value) -> Value {
    serde_json::from_str(&text(r)).unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(r)))
}

#[test]
fn an_agent_goes_from_an_empty_design_to_a_gcode_preview_over_the_control_channel() {
    let win = Window::start(McpPolicy::default());
    let mut s = win.mcp();
    // The catalog.
    let list = json_of(&call(&mut s, "list_commands", json!({"filter": "shape.add", "compact": true})));
    assert!(list["commands"].as_array().unwrap().iter().any(|c| c["id"] == "shape.add_rect"), "{list}");
    let d = json_of(&call(&mut s, "describe_command", json!({"command": "machine.jog"})));
    assert_eq!(d["hardware"], true);
    assert_eq!(d["hardware_class"], "motion");
    // A shape edit, read back.
    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 10, "y": 10, "width": 40, "height": 20, "layer": 1}}));
    assert!(r["isError"] != true, "{}", text(&r));
    let design = json_of(&call(&mut s, "inspect_design", json!({})));
    assert_eq!(design["shape_count"], 1);
    assert_eq!(design["shapes"][0]["bbox"], json!([10.0, 10.0, 50.0, 30.0]));
    // Layer settings, a batch, undo.
    let b = call(
        &mut s,
        "batch",
        json!({"steps": [
            {"command": "layer.set_mode", "params": {"layer": 1, "mode": "line"}},
            {"command": "layer.set_param", "params": {"layer": 1, "field": "speed", "value": 30}},
            {"command": "layer.set_param", "params": {"layer": 1, "field": "max_power", "value": 60}},
        ]}),
    );
    assert!(b["isError"] != true, "{}", text(&b));
    let layer = json_of(&call(&mut s, "inspect_design", json!({"path": "layers.1"})));
    assert_eq!((layer["speed"].as_f64(), layer["max_power"].as_f64()), (Some(30.0), Some(60.0)));
    let u = call(&mut s, "undo", json!({}));
    assert!(u["isError"] != true, "{}", text(&u));
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "layers.1"})))["max_power"].as_f64(), Some(20.0));
    // The G-code preview, never sent.
    let p = call(&mut s, "preview_job", json!({"format": "gcode"}));
    assert!(p["isError"] != true, "{}", text(&p));
    let preview = json_of(&p);
    assert!(preview["result"]["gcode"].as_str().unwrap().contains("G1"), "{preview}");
    assert_eq!(preview["result"]["sent_to_machine"], false);
    // Measure.
    let m = json_of(&call(&mut s, "measure", json!({"target": "design"})));
    assert!((m["total_length_mm"].as_f64().unwrap() - 120.0).abs() < 0.01, "{m}");
    // The screenshot (headless: an offscreen drawing of the design).
    let shot = call(&mut s, "screenshot", json!({}));
    assert_eq!(shot["content"][0]["type"], "image", "{shot}");
    // Safety: a machine command is refused with advice, and never ran.
    let jog = call(&mut s, "execute", json!({"command": "machine.jog", "params": {"dx": 5}}));
    assert_eq!(jog["isError"], true);
    assert!(text(&jog).contains("denied") && text(&jog).contains("ask the user"), "{}", text(&jog));
    let stop = call(&mut s, "execute", json!({"command": "machine.stop"}));
    assert!(!text(&stop).contains("[denied]"), "stop is always allowed: {}", text(&stop));
    // The audit log names every call.
    let ev = json_of(&call(&mut s, "events", json!({})));
    let audit: Vec<&Value> = ev["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "audit").collect();
    assert!(audit.iter().any(|e| e["command"] == "machine.jog" && e["outcome"] == "denied"), "{ev}");
    assert!(audit.iter().any(|e| e["command"] == "shape.add_rect" && e["outcome"] == "ok"));
    assert!(audit.iter().all(|e| e["args_hash"].is_string()));
    // The user's switches were not reachable from here.
    let allowed = win.on_ui(|g| g.policy().get().allow_hardware);
    assert!(!allowed);
}

#[test]
fn the_confirmation_dialog_flow_works_over_the_channel() {
    let win = Window::start(McpPolicy { allow_hardware: true, ..McpPolicy::default() });
    let mut s = win.mcp();
    let r = call(&mut s, "execute", json!({"command": "job.start"}));
    assert_eq!(r["isError"], true);
    let msg = text(&r);
    assert!(msg.contains("confirmation"), "{msg}");
    // The id the dialog carries, as the window sees it.
    let open = win.on_ui(|g| g.open_confirmations());
    assert_eq!(open.len(), 1);
    let pid = open[0].0.clone();
    assert!(msg.contains(&pid), "{msg}");
    // Nothing happens until the user clicks.
    let waiting = call(&mut s, "pending", json!({"id": pid.clone(), "wait_ms": 20}));
    assert!(text(&waiting).contains("still running"), "{}", text(&waiting));
    let p2 = pid.clone();
    win.on_ui(move |g| g.resolve_confirmation(&p2, true).unwrap());
    let done = json_of(&call(&mut s, "pending", json!({"id": pid})));
    let token = done["result"]["confirmation_token"].as_str().unwrap().to_string();
    let go = call(&mut s, "execute", json!({"command": "job.start", "confirmation_token": token}));
    assert!(!text(&go).contains("confirmation_invalid"), "{}", text(&go));
}

#[test]
fn a_wrong_token_never_gets_a_backend() {
    let win = Window::start(McpPolicy::default());
    let mut bad = RemoteBackend::new(Connect::Direct { port: win.port, token: "wrong".into() });
    let e = bad.list_commands(&ListQuery::default()).unwrap_err();
    assert_eq!(e.code, ErrorCode::Transport);
    let ok = win.on_ui(|g| g.inner().inspect("design", 0).map(|v| v["shape_count"].clone()).ok());
    assert_eq!(ok, Some(json!(0)), "the design is untouched");
}

// ----- third bug hunt: side doors around the gate ------------------------------------------------------

/// Ask for `id`, let the user click Confirm, and return the token.
fn confirmed_token(gate: &mut Gate<Spy>, id: &str, args: Value) -> String {
    let e = gate.execute(id, args).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{}", e.message);
    let pid = e.pending_id.unwrap();
    gate.resolve_confirmation(&pid, true).unwrap();
    let PendingState::Done { result: Ok(v), .. } = gate.pending(&pid).unwrap() else { panic!("confirmed") };
    v["confirmation_token"].as_str().unwrap().to_string()
}

#[test]
fn a_job_start_the_user_confirmed_does_not_start_a_design_changed_since() {
    let policy = McpPolicy { allow_hardware: true, ..McpPolicy::default() };
    let start = |gate: &mut Gate<Spy>, token: &str| {
        gate.execute_ctx("job.start", json!({}), &CallContext { tool: "execute".into(), confirmation_token: Some(token.to_string()) })
    };
    // The design changes after the user's click: a new shape, more power, another device.
    let changes: [(&str, Value); 4] = [
        ("shape.add_rect", json!({"x": 300, "y": 300, "width": 50, "height": 50})),
        ("layer.set_param", json!({"layer": 0, "field": "max_power", "value": 100})),
        ("layer.set_cut_settings", json!({"layer": 0, "settings": {"passes": 9}})),
        ("job.set_origin", json!({"start_from": "current_position"})),
    ];
    for (change, args) in changes {
        let (mut gate, seen, _app) = spy_gate(policy);
        gate.execute("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})).unwrap();
        let token = confirmed_token(&mut gate, "job.start", json!({}));
        gate.execute(change, args).unwrap_or_else(|e| panic!("{change}: {}", e.message));
        seen.borrow_mut().clear();
        let e = start(&mut gate, &token).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfirmationInvalid, "{change}: {}", e.message);
        assert!(seen.borrow().is_empty(), "{change}: the changed job reached the machine code");
    }
    // Another device selected after the click.
    let (mut gate, seen, app) = spy_gate(policy);
    let other = app.borrow_mut().machine.devices.add(crate::devices::Device::new(crate::devices::DeviceType::ALL[0]));
    let token = confirmed_token(&mut gate, "job.start", json!({}));
    app.borrow_mut().machine.select_device(other);
    let e = start(&mut gate, &token).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationInvalid, "{}", e.message);
    assert!(seen.borrow().is_empty());
    // Nothing changed (looking at the design is not a change): the token starts the job.
    let (mut gate, seen, _app) = spy_gate(policy);
    gate.execute("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})).unwrap();
    let token = confirmed_token(&mut gate, "job.start", json!({}));
    gate.inspect("design", 2).unwrap();
    seen.borrow_mut().clear();
    let _ = start(&mut gate, &token);
    assert_eq!(seen.borrow().as_slice(), ["job.start"]);
}

/// Connect the GRBL simulator and start a slow job on it, the way the user would from the window.
fn slow_job_on_the_simulator(app: &SharedApp) {
    let mut a = app.borrow_mut();
    let id = a.machine.devices.devices.iter().find(|d| d.kind == crate::devices::DeviceType::SimulatorGrbl).unwrap().id;
    crate::api::execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
    crate::api::execute_headless(&mut a, "machine.connect", &json!({})).unwrap();
    drop(a);
    wait_state(app, "ready", |s| s == crate::machine_api::LaserState::Ready);
    let lines = ["G21 G90".to_owned(), "M4 S100".to_owned()].into_iter().chain((0..200).map(|i| format!("G1 X{} Y10 F300", 10 + (i % 2) * 30)));
    let mut job = crate::machine_api::JobRequest::gcode(lines.collect());
    job.name = "slow".into();
    app.borrow_mut().machine.begin_start(job).unwrap();
    wait_state(app, "busy", |s| s == crate::machine_api::LaserState::Busy);
}

fn machine_state(app: &SharedApp) -> crate::machine_api::LaserState {
    let mut a = app.borrow_mut();
    a.machine.tick();
    a.machine.handle.snapshot().state
}

fn wait_state(app: &SharedApp, what: &str, f: impl Fn(crate::machine_api::LaserState) -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(20);
    while !f(machine_state(app)) {
        assert!(std::time::Instant::now() < end, "timed out waiting for {what}; state {:?}", machine_state(app));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_always_allowed_pause_never_resumes_a_paused_job() {
    use crate::machine_api::LaserState;
    // Machines are off for agents: only stop, pause and emergency stop get through.
    let (mut gate, _seen, app) = spy_gate(McpPolicy::default());
    slow_job_on_the_simulator(&app);
    gate.execute("job.pause", json!({})).unwrap();
    wait_state(&app, "paused", |s| s == LaserState::Paused);
    // Pause again: the laser must stay off. Resuming is machine.resume, which is denied.
    for id in ["job.pause", "machine.pause", "job.pause"] {
        let _ = gate.execute(id, json!({}));
        for _ in 0..40 {
            assert_eq!(machine_state(&app), LaserState::Paused, "{id} set the paused job going again");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(gate.execute("machine.resume", json!({})).unwrap_err().code, ErrorCode::Denied);
    gate.execute("job.stop", json!({})).unwrap();
    wait_state(&app, "stopped", |s| matches!(s, LaserState::Ready | LaserState::Alarm));
}

#[test]
fn emergency_stop_is_open_to_an_agent_whatever_the_policy() {
    use crate::machine_api::LaserState;
    let mut b = backend();
    let c = b.describe_command("machine.emergency_stop").unwrap().unwrap();
    assert_eq!(c.hardware_class, Some(HardwareClass::Stop));
    let (mut gate, seen, app) = spy_gate(McpPolicy { allow_file_write: false, ..McpPolicy::default() });
    slow_job_on_the_simulator(&app);
    gate.execute("machine.emergency_stop", json!({})).unwrap_or_else(|e| panic!("emergency stop was refused: {}", e.message));
    assert_eq!(seen.borrow().as_slice(), ["machine.emergency_stop"]);
    wait_state(&app, "stopped", |s| matches!(s, LaserState::Ready | LaserState::Alarm));
}

#[test]
fn an_agent_cannot_ask_for_a_real_connection_through_reset_connection() {
    let app = shared();
    {
        let mut a = app.borrow_mut();
        let id = a.machine.devices.add(crate::devices::Device {
            name: "Real".into(),
            connection: crate::devices::ConnectionKind::Tcp,
            host: "192.168.1.77".into(),
            ..crate::devices::Device::new(crate::devices::DeviceType::Grbl)
        });
        a.machine.devices.select(id);
    }
    // Even with machines allowed, an agent only connects simulators (as machine.connect says).
    let mut b = McpBackend::new(app.clone(), egui::Context::default(), false);
    for id in ["machine.connect", "machine.reset_connection"] {
        let e = b.execute(id, json!({})).unwrap_err();
        assert!(e.message.contains("simulator"), "{id}: {}", e.message);
        assert!(app.borrow().machine.dlg.confirm_connect.is_none(), "{id} put the Connect question for a real machine on the user's screen");
    }
}

#[test]
fn device_setup_is_closed_to_an_agent_until_machines_are_allowed() {
    // Machines are off for agents. What a job sends (start and end script, S range), where a
    // device connects, which device is the active one and the galvo timings are the machine's
    // setup, not the design: none of it may change.
    let (mut gate, seen, app) = spy_gate(McpPolicy::default());
    let sim = app.borrow().machine.devices.devices.iter().find(|d| d.is_simulator()).unwrap().id;
    let before = (app.borrow().job.gcode.clone(), app.borrow().machine.devices.devices.len(), app.borrow().machine.devices.active);
    let tries: &[(&str, Value)] = &[
        ("job.set_gcode_settings", json!({"start_script": "M3 S1000", "s_max": 1000000})),
        ("device.set_connection", json!({"host": "192.168.1.50", "port": 23})),
        ("device.import", json!({"text": "{}"})),
        ("device.add", json!({"type": "GRBL"})),
        ("device.duplicate", json!({"id": sim})),
        ("device.remove", json!({"id": sim})),
        ("device.select", json!({"id": sim})),
        ("device.adopt_detected", json!({})),
        ("device.bridge_setup", json!({"host": "192.168.1.50"})),
        ("device.read_motion", json!({})),
        ("galvo.settings_set", json!({"values": {}})),
        ("galvo.import_ezcad", json!({"path": "x.cfg"})),
        ("galvo.repeat_marking", json!({"count": 100})),
    ];
    for (id, args) in tries {
        let c = gate.describe_command(id).unwrap().unwrap_or_else(|| panic!("{id} is registered"));
        assert_eq!(c.hardware_class, Some(HardwareClass::Config), "{id}");
        let e = gate.execute(id, args.clone()).expect_err(&format!("{id} must not run by default"));
        assert_eq!(e.code, ErrorCode::Denied, "{id}: {}", e.message);
    }
    assert!(seen.borrow().is_empty(), "nothing reached the backend: {:?}", seen.borrow());
    let after = (app.borrow().job.gcode.clone(), app.borrow().machine.devices.devices.len(), app.borrow().machine.devices.active);
    assert!(before == after, "the device setup changed");
    // Reading it stays open.
    for id in ["device.list", "device.detected", "gcode.scripts", "device.automation_status"] {
        gate.execute(id, json!({})).unwrap_or_else(|e| panic!("{id}: {}", e.message));
    }
}

#[test]
fn an_agent_cannot_cut_the_users_connection_by_selecting_another_device() {
    use crate::machine_api::LaserState;
    let (mut gate, _seen, app) = spy_gate(McpPolicy::default());
    slow_job_on_the_simulator(&app);
    let other = app.borrow().machine.devices.devices.iter().find(|d| Some(d.id) != app.borrow().machine.devices.active).unwrap().id;
    let _ = gate.execute("device.select", json!({ "id": other }));
    for _ in 0..40 {
        assert_eq!(machine_state(&app), LaserState::Busy, "the running job lost its machine");
        std::thread::sleep(Duration::from_millis(5));
    }
    gate.execute("job.stop", json!({})).unwrap();
    wait_state(&app, "stopped", |s| matches!(s, LaserState::Ready | LaserState::Alarm));
}

#[test]
fn showing_the_job_log_folder_counts_as_writing_files() {
    // The folder is the person's setting; showing it creates it.
    let dir = std::env::temp_dir().join(format!("darkburn-hunt3-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut gate, _seen, app) = spy_gate(McpPolicy { allow_file_write: false, ..McpPolicy::default() });
    app.borrow_mut().prefs.opts.job_log_dir = dir.join("made/by/agent").to_string_lossy().into_owned();
    let e = gate.execute("job.log_show", json!({})).expect_err("file writes are off");
    assert_eq!(e.code, ErrorCode::Denied, "{}", e.message);
    assert!(!dir.exists(), "a folder was made with file writes off");
    let _ = crate::misc_cmds::take_revealed();
}

#[test]
fn huge_numbers_from_an_agent_never_leave_a_design_that_cannot_be_saved() {
    let big = 1e308;
    let tries: Vec<(&str, Value)> = vec![
        ("shape.add_rect", json!({"x": big, "y": 0, "width": big, "height": 10})),
        ("shape.add_rect", json!({"x": -big, "y": -big, "width": big, "height": big})),
        ("shape.add_ellipse", json!({"x": big, "y": big, "width": big, "height": big})),
        ("shape.add_polygon", json!({"cx": big, "cy": -big, "sides": 5, "radius": big})),
        ("shape.add_path", json!({"svg_path": "M0 0 L1e308 1e308 L-1e308 5 Z"})),
        ("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})),
        ("shape.translate", json!({"dx": big, "dy": big})),
        ("shape.translate", json!({"dx": big, "dy": big})),
        ("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})),
        ("shape.place", json!({"x": big, "y": -big})),
        ("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})),
        ("shape.resize", json!({"width": big, "height": big})),
        ("shape.resize", json!({"width": 1e-300, "height": 1e-300})),
        ("shape.add_rect", json!({"x": 10, "y": 10, "width": 20, "height": 20})),
        ("shape.rotate", json!({"degrees": big})),
        ("shape.set_props", json!({"power_scale": big, "priority": 9007199254740991i64, "start_point": [big, big]})),
        ("shape.set_corner_radius", json!({"radius": big})),
        ("layer.set_param", json!({"layer": 0, "field": "speed", "value": big})),
        ("layer.set_param", json!({"layer": 0, "field": "interval", "value": 1e-300})),
        ("layer.set_cut_settings", json!({"layer": 0, "settings": {"speed": big, "interval": 1e-300, "passes": 4000000000u64}})),
    ];
    let mut b = backend();
    let app = b.app_for_test();
    for (id, args) in &tries {
        let r = b.execute(id, args.clone());
        let a = app.borrow();
        for s in a.editor.doc.iter() {
            let bb = s.bbox();
            assert!(
                bb.x0.is_finite() && bb.y0.is_finite() && bb.x1.is_finite() && bb.y1.is_finite(),
                "{id} {args} (answered {r:?}) left a shape with no real size or place: {bb:?}"
            );
        }
    }
    // What is left saves and opens again as the same design.
    let dir = std::env::temp_dir().join(format!("darkburn-hunt3-huge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("huge.dbrn");
    b.execute("file.save_as", json!({"path": file.to_string_lossy()})).unwrap();
    let saved = app.borrow().editor.doc.clone();
    b.execute("file.open", json!({"path": file.to_string_lossy()})).unwrap_or_else(|e| panic!("the saved design does not open: {}", e.message));
    assert_eq!(app.borrow().editor.doc.len(), saved.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_region_screenshot_of_a_window_with_no_pixels_is_an_error_not_a_crash() {
    // A window that is minimized or zero pixels high hands over an empty picture.
    let region = Some(Region { x: 10.0, y: 10.0, width: 50.0, height: 50.0 });
    for size in [[0, 0], [0, 40], [40, 0]] {
        let empty = egui::ColorImage::filled(size, egui::Color32::BLACK);
        assert!(crate::mcp_backend::png_of(&empty, region, 1.0).is_err(), "{size:?}");
        assert!(crate::mcp_backend::png_of(&empty, None, 1.0).is_err(), "{size:?}");
    }
    let small = egui::ColorImage::filled([40, 30], egui::Color32::BLACK);
    assert!(crate::mcp_backend::png_of(&small, region, 1.0).is_ok());
}

/// Restoring an agent's checkpoint puts another editor in place, the way New and Open do: the
/// windows that hold shapes or settings of the replaced one close, so their OK cannot write the
/// old state over the restored document (Edit Text, Convert to Bitmap, the shape tool windows).
#[test]
fn restoring_a_checkpoint_closes_the_windows_that_held_the_old_shapes() {
    let app = shared();
    let mut b = McpBackend::new(app.clone(), egui::Context::default(), true);
    exec(&mut b, "shape.add_rect", json!({"x": 10, "y": 10, "width": 40, "height": 20, "layer": 0})).unwrap();
    let cp = b.checkpoint().unwrap().expect("a checkpoint");
    exec(&mut b, "shape.add_rect", json!({"x": 60, "y": 10, "width": 40, "height": 20, "layer": 0})).unwrap();
    let serial = {
        let mut a = app.borrow_mut();
        a.text.edit.open = true;
        a.text.qr.open = true;
        a.shape_ui.open = Some("tools.boolean_assistant");
        a.files.project_serial
    };
    b.restore(&cp).unwrap();
    let a = app.borrow();
    assert_eq!(a.editor.doc.len(), 1, "the checkpoint's document is back");
    assert!(!a.text.edit.open && !a.text.qr.open, "the text windows still hold the replaced document");
    assert!(a.shape_ui.open.is_none(), "the Boolean Assistant still holds the replaced document");
    assert!(a.files.project_serial > serial, "work started on the replaced document is not cancelled");
}
