//! The Gweike card simulator commands as agents and the window see them: their safety classes,
//! how the Gweike switch and those classes combine in the MCP gate, and the window's buttons.

use std::cell::RefCell;
use std::rc::Rc;

use darkburn_mcp::{
    Backend, CallContext, CommandError, CommandInfo, CommandPage, ErrorCode, EventPage, Gate, HardwareClass, ListQuery, McpPolicy, PolicyHandle,
};
use serde_json::{Value, json};

use crate::mcp_backend::{McpBackend, SharedApp};
use crate::mcp_catalog;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

/// Records what reaches the command registry behind the gate.
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

fn gate(policy: McpPolicy) -> (Gate<Spy>, Rc<RefCell<Vec<String>>>, SharedApp) {
    let app: SharedApp = Rc::new(RefCell::new(DarkBurn::new(Prefs::default())));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let spy = Spy { inner: McpBackend::new(app.clone(), egui::Context::default(), true), executed: seen.clone() };
    (Gate::new(spy, PolicyHandle::new(policy)), seen, app)
}

/// Machine control on, the Gweike switch off, no confirmation in the way.
fn machines_on_gweike_off() -> McpPolicy {
    McpPolicy { allow_hardware: true, allow_gweike: false, require_confirmation_for_job_start: false, allow_file_write: true }
}

#[test]
fn gweike_card_commands_have_their_own_classes() {
    for id in ["mcc.stop", "mcc.emergency_stop"] {
        assert_eq!(mcp_catalog::hardware_class(id), Some(HardwareClass::Stop), "{id}");
    }
    for id in ["mcc.connect", "mcc.disconnect", "mcc.status"] {
        assert_eq!(mcp_catalog::hardware_class(id), Some(HardwareClass::Connect), "{id}");
    }
    // The dissector only reads the bytes it is given: not hardware, no class.
    assert!(!mcp_catalog::is_hardware("mcc.dissect"));
    assert_eq!(mcp_catalog::hardware_class("mcc.dissect"), None);
    // An unknown card command still lands in the strictest class.
    assert_eq!(mcp_catalog::hardware_class("mcc.read_registers"), Some(HardwareClass::FireOrJob));
    // Every card hardware command is a Gweike command, whatever device is active.
    for id in ["mcc.connect", "mcc.disconnect", "mcc.status", "mcc.stop", "mcc.emergency_stop"] {
        assert!(mcp_catalog::is_hardware(id), "{id}");
        assert!(mcp_catalog::is_gweike(id, false), "{id}");
    }
}

#[test]
fn gweike_card_classes_reach_the_mcp_listing() {
    let (mut g, _seen, _app) = gate(McpPolicy::default());
    for (id, class) in [
        ("mcc.connect", Some(HardwareClass::Connect)),
        ("mcc.disconnect", Some(HardwareClass::Connect)),
        ("mcc.status", Some(HardwareClass::Connect)),
        ("mcc.stop", Some(HardwareClass::Stop)),
        ("mcc.emergency_stop", Some(HardwareClass::Stop)),
        ("mcc.dissect", None),
    ] {
        let info = g.describe_command(id).unwrap().unwrap_or_else(|| panic!("{id} is listed"));
        assert_eq!(info.hardware_class, class, "{id}");
        assert_eq!(info.effective_class(), class, "{id}");
        assert!(info.gweike, "{id} is flagged Gweike");
    }
}

#[test]
fn mcp_gweike_switch_off_refuses_card_connect_with_machine_control_on() {
    let (mut g, seen, app) = gate(machines_on_gweike_off());
    for id in ["mcc.connect", "mcc.disconnect", "mcc.status"] {
        let e = g.execute(id, json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied, "{id}: {}", e.message);
        assert!(e.message.contains("Gweike"), "{id}: the refusal names the Gweike switch: {}", e.message);
    }
    assert!(seen.borrow().is_empty(), "nothing reached the registry: {:?}", seen.borrow());
    assert!(!app.borrow().machine.ext.mcc.connected());
    // With both switches on, connect reaches the simulator.
    let mut p = machines_on_gweike_off();
    p.allow_gweike = true;
    let (mut g, seen, app) = gate(p);
    g.execute("mcc.connect", json!({})).unwrap();
    assert_eq!(seen.borrow().as_slice(), ["mcc.connect"]);
    assert!(app.borrow().machine.ext.mcc.connected());
}

#[test]
fn mcp_gweike_card_connect_needs_machine_control_too() {
    let p = McpPolicy { allow_gweike: true, ..McpPolicy::default() };
    let (mut g, seen, _app) = gate(p);
    let e = g.execute("mcc.connect", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied, "{}", e.message);
    assert!(seen.borrow().is_empty());
}

#[test]
fn mcp_gweike_card_stops_pass_as_other_stops_do() {
    for policy in [McpPolicy::default(), machines_on_gweike_off()] {
        let (mut g, seen, _app) = gate(policy);
        for id in ["machine.stop", "mcc.stop", "mcc.emergency_stop"] {
            let r = g.execute(id, json!({}));
            if let Err(e) = &r {
                assert_ne!(e.code, ErrorCode::Denied, "{id} was refused by the gate: {}", e.message);
                assert_ne!(e.code, ErrorCode::ConfirmationRequired, "{id} asked for a confirmation");
            }
        }
        assert_eq!(seen.borrow().as_slice(), ["machine.stop", "mcc.stop", "mcc.emergency_stop"], "every stop reached the registry");
    }
}

#[test]
fn mcp_gweike_card_dissect_needs_no_switch() {
    let (mut g, seen, _app) = gate(McpPolicy::default());
    let req = darkburn_mcc::Request::read(1000, 2).unwrap().encode(7).unwrap();
    let hex: String = req.iter().map(|b| format!("{b:02x}")).collect();
    let v = g.execute("mcc.dissect", json!({ "hex": hex })).unwrap();
    assert_eq!(v["seq"], 7, "{v}");
    assert_eq!(seen.borrow().as_slice(), ["mcc.dissect"]);
}

#[test]
fn gweike_card_window_answer_reads_as_text() {
    let mut a = DarkBurn::new(Prefs::default());
    let req = darkburn_mcc::Request::read(1000, 2).unwrap().encode(7).unwrap();
    let hex: String = req.iter().map(|b| format!("{b:02x} ")).collect();
    let d = crate::api::execute_headless(&mut a, "mcc.dissect", &json!({ "hex": hex })).unwrap();
    assert_eq!(crate::gweike_ui::card_answer(&d), d["text"].as_str().unwrap());
    let c = crate::api::execute_headless(&mut a, "mcc.connect", &json!({})).unwrap();
    let s = crate::api::execute_headless(&mut a, "mcc.status", &json!({})).unwrap();
    for v in [&c, &s] {
        let t = crate::gweike_ui::card_answer(v);
        assert!(!t.is_empty());
        match v.get("text").and_then(Value::as_str) {
            // A summary in words: the window shows it as it is.
            Some(words) => assert_eq!(t, words),
            None => {
                for k in v.as_object().unwrap().keys() {
                    assert!(t.contains(k.as_str()), "{k} missing from {t}");
                }
            }
        }
    }
    assert_eq!(crate::gweike_ui::card_answer(&json!({ "a": 1, "b": "x" })), "a: 1\nb: \"x\"");
    assert_eq!(crate::gweike_ui::card_answer(&json!({})), "{}");
}

#[test]
fn gweike_card_window_buttons_name_registered_commands_only() {
    let src = include_str!("gweike_ui.rs");
    let code = src.split("#[cfg(test)]").next().unwrap();
    let ids: Vec<&str> = code.split('"').filter(|s| s.starts_with("mcc.") && s.len() > 4).collect();
    for want in ["mcc.connect", "mcc.disconnect", "mcc.status", "mcc.stop", "mcc.emergency_stop", "mcc.dissect"] {
        assert!(ids.contains(&want), "the window has no button for {want}");
    }
    for id in &ids {
        assert!(crate::commands::find(id).is_some(), "{id} is not a registered command");
    }
    // The window starts nothing that moves or fires the machine.
    for f in ["mcc.jog", "mcc.home", "mcc.move", "mcc.output", "mcc.start", "machine.jog", "machine.home", "machine.start", "job.start"] {
        assert!(!code.contains(&format!("\"{f}")), "the Gweike window mentions {f}");
    }
    // It holds no copy of the card logic: no link to the simulator, only the registry.
    for f in ["SimLink", "mcc::sim", "gweike_card::run", "UdpSocket", "TcpStream", "serialport"] {
        assert!(!code.contains(f), "the Gweike window mentions {f}");
    }
}
