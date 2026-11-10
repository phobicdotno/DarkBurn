//! Side doors around the safety gate found in the third bug hunt: each test drives the gate over
//! a small backend whose state the commands change.

use darkburn_mcp::{
    Backend, BatchStep, CallContext, CommandError, CommandInfo, CommandPage, ErrorCode, Gate, HardwareClass, ListQuery, McpPolicy, PendingState,
    PolicyHandle,
};
use serde_json::{Value, json};

/// A backend with a design that changes, a device that can become a Gweike one, and a console.
#[derive(Default)]
struct Probe {
    gweike: bool,
    revision: u32,
    ran: Vec<String>,
    dialogs: Vec<String>,
}

impl Probe {
    fn catalog(&self) -> Vec<CommandInfo> {
        let hardware = |id: &str, class: HardwareClass| {
            let mut c = CommandInfo::new(id, id, "Laser");
            c.hardware = true;
            c.hardware_class = Some(class);
            c
        };
        let mut connect = hardware("machine.connect", HardwareClass::Connect);
        connect.gweike = self.gweike;
        let mut console = hardware("machine.console", HardwareClass::FireOrJob);
        console.params_schema =
            json!({"type": "object", "properties": {"line": {"type": "string"}}, "required": ["line"], "additionalProperties": false});
        vec![
            CommandInfo::new("device.select_gweike", "Select the Gweike device", "Devices"),
            CommandInfo::new("design.edit", "Change the design", "Edit"),
            connect,
            hardware("job.start", HardwareClass::FireOrJob),
            console,
        ]
    }
}

impl Backend for Probe {
    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError> {
        Ok(query.apply(self.catalog()))
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        Ok(self.catalog().into_iter().find(|c| c.id == id))
    }
    fn execute(&mut self, id: &str, _args: Value) -> Result<Value, CommandError> {
        self.ran.push(id.to_string());
        match id {
            "device.select_gweike" => self.gweike = true,
            "design.edit" => self.revision += 1,
            _ => {}
        }
        Ok(json!({"ok": true}))
    }
    fn inspect(&mut self, _path: &str, _depth: u32) -> Result<Value, CommandError> {
        Ok(Value::Null)
    }
    fn confirmation_scope(&mut self) -> String {
        format!("revision {}", self.revision)
    }
    fn request_confirmation(&mut self, _pending_id: &str, _command: &str, summary: &str) -> Result<(), CommandError> {
        self.dialogs.push(summary.to_string());
        Ok(())
    }
}

fn gate(policy: McpPolicy) -> Gate<Probe> {
    Gate::new(Probe::default(), PolicyHandle::new(policy))
}

fn machines_allowed() -> McpPolicy {
    McpPolicy { allow_hardware: true, allow_file_write: true, require_confirmation_for_job_start: true, allow_gweike: false }
}

fn step(command: &str, token: Option<&str>) -> BatchStep {
    BatchStep { command: command.into(), params: json!({}), confirmation_token: token.map(str::to_string) }
}

fn with_token(g: &mut Gate<Probe>, id: &str, args: Value, token: &str) -> Result<Value, CommandError> {
    g.execute_ctx(id, args, &CallContext { tool: "execute".into(), confirmation_token: Some(token.into()) })
}

/// Ask, let the user click Confirm, and fetch the token.
fn confirmed(g: &mut Gate<Probe>, id: &str, args: Value) -> String {
    let e = g.execute(id, args).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{e}");
    let pid = e.pending_id.unwrap();
    g.resolve_confirmation(&pid, true).unwrap();
    match g.pending(&pid).unwrap() {
        PendingState::Done { result: Ok(v), .. } => v["confirmation_token"].as_str().unwrap().to_string(),
        other => panic!("expected a token, got {other:?}"),
    }
}

#[test]
fn a_batch_cannot_select_a_gweike_machine_and_then_drive_it() {
    // Machines are allowed, Gweike machines are not. On its own the connect is fine while
    // another device is active, and refused once the Gweike one is.
    let mut g = gate(machines_allowed());
    let r = g.batch(&[step("device.select_gweike", None), step("machine.connect", None)], false).unwrap();
    assert!(!g.inner().ran.contains(&"machine.connect".to_string()), "the connect reached the Gweike machine: {:?}", g.inner().ran);
    assert!(!r.ok);
    assert_eq!(r.failed_step, Some(1));
    assert_eq!(r.steps[1].error.as_ref().unwrap().code, ErrorCode::Denied);
}

#[test]
fn a_confirmation_is_only_good_for_the_design_the_user_saw() {
    let mut g = gate(machines_allowed());
    let token = confirmed(&mut g, "job.start", json!({}));
    // The agent changes the design after the user said yes, then starts with the token.
    g.execute("design.edit", json!({})).unwrap();
    let e = with_token(&mut g, "job.start", json!({}), &token).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationInvalid, "{e}");
    assert!(!g.inner().ran.contains(&"job.start".to_string()), "the changed design was started");
    // Unchanged, the token works.
    let mut g = gate(machines_allowed());
    let token = confirmed(&mut g, "job.start", json!({}));
    with_token(&mut g, "job.start", json!({}), &token).unwrap();
}

#[test]
fn a_batch_cannot_change_the_design_between_the_confirmation_and_the_start() {
    let mut g = gate(machines_allowed());
    let token = confirmed(&mut g, "job.start", json!({}));
    let r = g.batch(&[step("design.edit", None), step("job.start", Some(&token))], false).unwrap();
    assert!(!g.inner().ran.contains(&"job.start".to_string()), "the changed design was started: {:?}", g.inner().ran);
    assert_eq!(r.failed_step, Some(1));
    assert_eq!(r.steps[1].error.as_ref().unwrap().code, ErrorCode::ConfirmationInvalid);
}

#[test]
fn the_user_is_shown_all_of_what_they_confirm_or_is_not_asked() {
    // A console line whose dangerous end is past the first couple of hundred characters.
    let mut g = gate(machines_allowed());
    let line = format!("G0 X0{}M3 S1000", " ".repeat(300));
    let e = g.execute("machine.console", json!({"line": line})).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{e}");
    assert!(g.inner().dialogs[0].contains("M3 S1000"), "the dialog hides the end of the line: {}", g.inner().dialogs[0]);
    // Too long to show: refused, and no dialog opens.
    let mut g = gate(machines_allowed());
    let e = g.execute("machine.console", json!({"line": format!("G0 X0{}M3 S1000", " ".repeat(5000))})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied, "{e}");
    assert!(g.inner().dialogs.is_empty());
    assert!(g.open_confirmations().is_empty());
}

#[test]
fn asking_again_and_again_does_not_stack_dialogs() {
    let mut g = gate(machines_allowed());
    let first = g.execute("job.start", json!({})).unwrap_err().pending_id.unwrap();
    for _ in 0..50 {
        let e = g.execute("job.start", json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::ConfirmationRequired);
        assert_eq!(e.pending_id.as_deref(), Some(first.as_str()), "the same request is the same dialog");
    }
    assert_eq!(g.inner().dialogs.len(), 1);
    // Different requests: a handful may wait, the rest are refused until the user answers.
    for i in 0..200 {
        let _ = g.execute("machine.console", json!({"line": format!("G0 X{i}")}));
    }
    let open = g.open_confirmations().len();
    assert!(open <= 8, "{open} dialogs wait for the user");
    assert_eq!(g.inner().dialogs.len(), open);
    let e = g.execute("machine.console", json!({"line": "G0 X999"})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied, "{e}");
    // Answering one makes room again.
    g.resolve_confirmation(&first, false).unwrap();
    assert_eq!(g.execute("machine.console", json!({"line": "G0 X999"})).unwrap_err().code, ErrorCode::ConfirmationRequired);
}

#[test]
fn names_from_the_client_cannot_forge_or_flood_the_log() {
    let mut g = gate(McpPolicy::default());
    let forged = "execute job.stop  ok\n12:00:00  audit      execute machine.fire  ok";
    let _ = g.execute_ctx("design.edit", json!({}), &CallContext { tool: forged.into(), confirmation_token: None });
    let _ = g.execute(&format!("no.such\r\n{}", "x".repeat(1 << 20)), json!({}));
    let _ = g.inspect(&format!("design.\u{2028}{}", "y".repeat(1 << 20)), 0);
    let events = g.events(0).unwrap().events;
    assert_eq!(events.len(), 3);
    for e in &events {
        for text in [&e.tool, &e.command] {
            assert!(!text.chars().any(|c| c.is_control() || c == '\u{2028}'), "a line break in the log: {text:?}");
            assert!(text.chars().count() <= 130, "{} characters in one log name", text.chars().count());
        }
    }
    assert!(events[0].tool.starts_with("execute job.stop"), "the text itself is kept: {}", events[0].tool);
}

// ----- the control channel ---------------------------------------------------------------------------

mod channel {
    use std::time::Duration;

    use darkburn_mcp::control::{ControlClient, ControlConfig, ControlServer, read_frame, write_frame};
    use darkburn_mcp::{ErrorCode, Gate, McpPolicy, MockBackend, PolicyHandle};
    use serde_json::json;

    const TOKEN: &str = "hunt3-token-0123456789abcdef";

    fn server(timeout: Duration) -> ControlServer {
        ControlServer::bind(ControlConfig { port: 0, token: Some(TOKEN.into()), request_timeout: timeout, ..Default::default() }).unwrap()
    }

    #[test]
    fn a_request_the_client_was_told_timed_out_does_not_run_later() {
        // The UI thread is busy (a system file dialog is open, say) and serves nothing.
        let server = server(Duration::from_millis(200));
        let mut c = ControlClient::connect(server.addr(), TOKEN).unwrap();
        let rect = json!({"id": "shape.add_rect", "args": {"x": 0, "y": 0, "width": 5, "height": 5}});
        let e = c.call("execute", rect).unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout, "{e}");
        // The UI thread comes back. The agent was told the call failed, so it must not happen now.
        let mut gate = Gate::new(MockBackend::new(), PolicyHandle::new(McpPolicy::default()));
        assert_eq!(server.pump(&mut gate, 64), 0);
        assert!(gate.inner().executed.is_empty(), "ran after the timeout: {:?}", gate.inner().executed);
    }

    #[test]
    fn a_request_of_a_client_that_went_away_does_not_run_later() {
        let server = server(Duration::from_secs(600));
        let mut s = std::net::TcpStream::connect(server.addr()).unwrap();
        write_frame(&mut s, &json!({"hello": {"token": TOKEN, "version": 1}})).unwrap();
        assert_eq!(read_frame(&mut s, 4096).unwrap().unwrap()["ok"], true);
        let rect = json!({"id": "shape.add_rect", "args": {"x": 0, "y": 0, "width": 5, "height": 5}});
        write_frame(&mut s, &json!({"id": 1, "method": "execute", "params": rect})).unwrap();
        // The agent is killed before the UI thread got to its request.
        drop(s);
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while server.connections() > 0 && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(server.connections(), 0, "the dead client's connection is still held");
        let mut gate = Gate::new(MockBackend::new(), PolicyHandle::new(McpPolicy::default()));
        assert_eq!(server.pump(&mut gate, 64), 0);
        assert!(gate.inner().executed.is_empty(), "ran for a client that is gone: {:?}", gate.inner().executed);
    }
}
