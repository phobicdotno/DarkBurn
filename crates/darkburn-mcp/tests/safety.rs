//! Each rule of the safety gate, and the proof that a denied command never reaches the backend.

mod common;

use std::time::Duration;

use common::*;
use darkburn_mcp::{Backend, BatchStep, ErrorCode, McpPolicy, PendingState};
use serde_json::{Value, json};

fn exec(g: &mut impl Backend, id: &str, args: Value) -> Result<Value, darkburn_mcp::CommandError> {
    g.execute(id, args)
}

fn exec_tok(g: &mut impl Backend, id: &str, args: Value, tok: &str) -> Result<Value, darkburn_mcp::CommandError> {
    g.execute_ctx(id, args, &darkburn_mcp::CallContext { tool: "execute".into(), confirmation_token: Some(tok.into()) })
}

/// Walk the confirmation flow up to the token, like the human clicking Confirm.
fn confirmed_token(g: &mut darkburn_mcp::Gate<darkburn_mcp::MockBackend>, id: &str, args: Value) -> String {
    let e = exec(g, id, args).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{e}");
    let pid = e.pending_id.unwrap();
    g.resolve_confirmation(&pid, true).unwrap();
    match g.pending(&pid).unwrap() {
        PendingState::Done { result: Ok(v), .. } => v["confirmation_token"].as_str().unwrap().to_string(),
        other => panic!("expected a token, got {other:?}"),
    }
}

#[test]
fn defaults_match_the_spec() {
    let p = McpPolicy::default();
    assert!(!p.allow_hardware);
    assert!(!p.allow_gweike);
    assert!(p.require_confirmation_for_job_start);
    assert!(p.allow_file_write);
}

#[test]
fn hardware_is_denied_by_default_and_never_reaches_the_backend() {
    let mut g = gate(McpPolicy::default());
    for (id, args) in [
        ("machine.jog", json!({"dx": 1, "dy": 1})),
        ("machine.connect", json!({})),
        ("job.start", json!({})),
        ("laser.test_fire", json!({"power": 5})),
        ("gweike.read_card", json!({})),
        ("machine.unflagged", json!({})),
    ] {
        let e = exec(&mut g, id, args).unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied, "{id}: {e}");
        assert!(e.message.contains("Allow AI agents to control machines"), "{id}: message must tell the agent what to ask: {}", e.message);
    }
    assert!(g.inner().executed.is_empty(), "a denied command reached the backend: {:?}", g.inner().executed);
    assert!(g.inner().confirmations_requested.is_empty(), "no dialog for a denied command");
}

#[test]
fn denial_through_the_mcp_tool_names_the_setting() {
    let mut s = default_server();
    let r = call(&mut s, "execute", json!({"command": "machine.jog", "params": {"dx": 1, "dy": 0}}));
    assert!(is_error(&r));
    assert!(text(&r).contains("[denied]") && text(&r).contains("ask the user"), "{}", text(&r));
    let b = call(&mut s, "batch", json!({"steps": [{"command": "machine.jog", "params": {"dx": 1, "dy": 0}}]}));
    assert!(is_error(&b) && text(&b).contains("denied"), "{}", text(&b));
}

#[test]
fn emergency_stop_pause_and_stop_are_always_allowed() {
    let mut g = gate(McpPolicy::default());
    exec(&mut g, "machine.emergency_stop", json!({})).unwrap();
    exec(&mut g, "machine.pause", json!({})).unwrap();
    assert_eq!(g.inner().executed, ["machine.emergency_stop", "machine.pause"]);
    // Also with everything locked down explicitly, and inside a batch.
    let mut g = gate(McpPolicy { allow_hardware: false, allow_file_write: false, require_confirmation_for_job_start: true, allow_gweike: false });
    let b = g.batch(&[BatchStep { command: "machine.emergency_stop".into(), params: json!({}), confirmation_token: None }], true).unwrap();
    assert!(b.ok);
}

#[test]
fn hardware_runs_once_the_human_allows_it() {
    let mut g = gate(McpPolicy::default());
    assert!(exec(&mut g, "machine.jog", json!({"dx": 1, "dy": 1})).is_err());
    g.policy().update(|p| p.allow_hardware = true);
    exec(&mut g, "machine.jog", json!({"dx": 1, "dy": 1})).unwrap();
    exec(&mut g, "machine.connect", json!({})).unwrap();
    // Switching it off again takes effect on the next call.
    g.policy().update(|p| p.allow_hardware = false);
    assert_eq!(exec(&mut g, "machine.jog", json!({"dx": 1, "dy": 1})).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(g.inner().executed, ["machine.jog", "machine.connect"]);
}

#[test]
fn unspecified_hardware_class_is_treated_as_hardware() {
    let mut g = gate(allow_all());
    exec(&mut g, "machine.unflagged", json!({})).unwrap(); // allowed when hardware is on, no confirmation (not job/fire)
    let mut g = gate(McpPolicy::default());
    assert_eq!(exec(&mut g, "machine.unflagged", json!({})).unwrap_err().code, ErrorCode::Denied);
}

#[test]
fn gweike_needs_its_own_switch_on_top_of_hardware() {
    let mut g = gate(McpPolicy { allow_hardware: true, ..McpPolicy::default() });
    let e = exec(&mut g, "gweike.read_card", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied);
    assert!(e.message.contains("Gweike"), "{e}");
    let e = exec(&mut g, "gweike.job_start", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied, "gweike denial comes before confirmation");
    assert!(g.inner().executed.is_empty());
    assert!(g.inner().confirmations_requested.is_empty());
    // allow_gweike alone, without hardware, still denies.
    let mut g = gate(McpPolicy { allow_gweike: true, ..McpPolicy::default() });
    assert_eq!(exec(&mut g, "gweike.read_card", json!({})).unwrap_err().code, ErrorCode::Denied);
    // Both on: allowed.
    let mut g = gate(McpPolicy { allow_hardware: true, allow_gweike: true, ..McpPolicy::default() });
    exec(&mut g, "gweike.read_card", json!({})).unwrap();
    assert_eq!(g.inner().executed, ["gweike.read_card"]);
}

#[test]
fn job_start_needs_a_confirmation_token_from_the_human() {
    let mut g = gate(allow_all());
    let e = exec(&mut g, "job.start", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    let pid = e.pending_id.clone().unwrap();
    assert!(e.message.contains("confirmation_token") && e.message.contains(&pid));
    assert!(g.inner().executed.is_empty(), "nothing ran before the click");
    assert_eq!(g.inner().confirmations_requested.len(), 1, "the app was asked to show its dialog");
    assert_eq!(g.inner().confirmations_requested[0].0, pid);
    assert_eq!(g.inner().confirmations_requested[0].1, "job.start");
    // Waiting for the click.
    assert!(matches!(g.pending(&pid).unwrap(), PendingState::Running { .. }));
    assert_eq!(g.running().unwrap().len(), 1);
    // The agent cannot mint a token by itself.
    assert_eq!(exec_tok(&mut g, "job.start", json!({}), "made-up").unwrap_err().code, ErrorCode::ConfirmationInvalid);
    assert!(g.inner().executed.is_empty());
    // The human clicks Confirm.
    g.resolve_confirmation(&pid, true).unwrap();
    let tok = match g.pending(&pid).unwrap() {
        PendingState::Done { result: Ok(v), .. } => v["confirmation_token"].as_str().unwrap().to_string(),
        o => panic!("{o:?}"),
    };
    exec_tok(&mut g, "job.start", json!({}), &tok).unwrap();
    assert_eq!(g.inner().executed, ["job.start"]);
    // One time only.
    let e = exec_tok(&mut g, "job.start", json!({}), &tok).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationInvalid);
    assert_eq!(g.inner().executed.len(), 1);
}

#[test]
fn a_token_is_bound_to_the_exact_command_and_arguments() {
    let mut g = gate(allow_all());
    let tok = confirmed_token(&mut g, "laser.test_fire", json!({"power": 5}));
    // Different arguments.
    assert_eq!(exec_tok(&mut g, "laser.test_fire", json!({"power": 50}), &tok).unwrap_err().code, ErrorCode::ConfirmationInvalid);
    // Different command.
    assert_eq!(exec_tok(&mut g, "job.start", json!({}), &tok).unwrap_err().code, ErrorCode::ConfirmationInvalid);
    assert!(g.inner().executed.is_empty());
    // The right one still works afterwards (failed attempts do not burn it).
    exec_tok(&mut g, "laser.test_fire", json!({"power": 5}), &tok).unwrap();
    assert_eq!(g.inner().executed, ["laser.test_fire"]);
}

#[test]
fn cancelling_the_dialog_denies_and_a_late_token_does_not_exist() {
    let mut g = gate(allow_all());
    let e = exec(&mut g, "job.start", json!({})).unwrap_err();
    let pid = e.pending_id.unwrap();
    g.resolve_confirmation(&pid, false).unwrap();
    match g.pending(&pid).unwrap() {
        PendingState::Done { result: Err(e), .. } => assert_eq!(e.code, ErrorCode::ConfirmationDenied),
        o => panic!("{o:?}"),
    }
    assert!(g.resolve_confirmation(&pid, true).is_err(), "a cancelled request cannot be approved afterwards");
    assert!(g.inner().executed.is_empty());
}

#[test]
fn tokens_expire() {
    let mut g = gate(allow_all());
    let tok = confirmed_token(&mut g, "job.start", json!({}));
    g.set_token_ttl(Duration::ZERO);
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(exec_tok(&mut g, "job.start", json!({}), &tok).unwrap_err().code, ErrorCode::ConfirmationInvalid);
    assert!(g.inner().executed.is_empty());
}

#[test]
fn confirmation_can_be_switched_off_by_the_human() {
    let mut g = gate(McpPolicy { allow_hardware: true, require_confirmation_for_job_start: false, ..McpPolicy::default() });
    exec(&mut g, "job.start", json!({})).unwrap();
    assert_eq!(g.inner().executed, ["job.start"]);
    // Still denied when hardware is off, whatever the confirmation setting says.
    g.policy().update(|p| p.allow_hardware = false);
    assert_eq!(exec(&mut g, "job.start", json!({})).unwrap_err().code, ErrorCode::Denied);
}

#[test]
fn no_dialog_means_no_job() {
    // A backend that cannot show the dialog must not let the job start.
    struct NoDialog(darkburn_mcp::MockBackend);
    impl Backend for NoDialog {
        fn list_commands(&mut self, q: &darkburn_mcp::ListQuery) -> Result<darkburn_mcp::CommandPage, darkburn_mcp::CommandError> {
            self.0.list_commands(q)
        }
        fn describe_command(&mut self, id: &str) -> Result<Option<darkburn_mcp::CommandInfo>, darkburn_mcp::CommandError> {
            self.0.describe_command(id)
        }
        fn execute(&mut self, id: &str, a: Value) -> Result<Value, darkburn_mcp::CommandError> {
            self.0.execute(id, a)
        }
        fn inspect(&mut self, p: &str, d: u32) -> Result<Value, darkburn_mcp::CommandError> {
            self.0.inspect(p, d)
        }
    }
    let mut g = darkburn_mcp::Gate::new(NoDialog(darkburn_mcp::MockBackend::new()), darkburn_mcp::PolicyHandle::new(allow_all()));
    let e = g.execute("job.start", json!({})).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied);
    assert!(g.inner().0.executed.is_empty());
}

#[test]
fn file_writes_need_their_own_permission() {
    let mut g = gate(McpPolicy { allow_file_write: false, ..McpPolicy::default() });
    assert_eq!(exec(&mut g, "file.save_as", json!({"path": "C:/x/a.db"})).unwrap_err().code, ErrorCode::Denied);
    assert_eq!(exec(&mut g, "file.export", json!({"path": "C:/x/a.svg"})).unwrap_err().code, ErrorCode::Denied);
    assert!(g.inner().executed.is_empty());
    // Reads and edits are fine.
    exec(&mut g, "file.open", json!({"path": "C:/x/a.db"})).unwrap();
    g.policy().update(|p| p.allow_file_write = true);
    exec(&mut g, "file.save_as", json!({"path": "C:/x/a.db"})).unwrap();
    assert_eq!(g.inner().executed, ["file.open", "file.save_as"]);
}

#[test]
fn a_denied_step_rejects_the_whole_batch_before_anything_runs() {
    let mut g = gate(McpPolicy::default());
    let steps = [
        BatchStep { command: "shape.add_rect".into(), params: json!({"x": 0, "y": 0, "width": 1, "height": 1}), confirmation_token: None },
        BatchStep { command: "machine.jog".into(), params: json!({"dx": 1, "dy": 1}), confirmation_token: None },
    ];
    let e = g.batch(&steps, true).unwrap_err();
    assert_eq!(e.code, ErrorCode::Denied);
    assert!(e.message.contains("Step 1") && e.message.contains("nothing ran"), "{e}");
    assert!(g.inner().executed.is_empty(), "step 0 must not have run");
    // Same with all_or_nothing = false.
    assert!(g.batch(&steps, false).is_err());
    assert!(g.inner().executed.is_empty());
}

#[test]
fn batch_steps_that_start_jobs_need_tokens_too() {
    let mut g = gate(allow_all());
    let mut steps = vec![BatchStep { command: "job.start".into(), params: json!({}), confirmation_token: None }];
    let e = g.batch(&steps, true).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    g.resolve_confirmation(e.pending_id.as_ref().unwrap(), true).unwrap();
    let tok = match g.pending(e.pending_id.as_ref().unwrap()).unwrap() {
        PendingState::Done { result: Ok(v), .. } => v["confirmation_token"].as_str().unwrap().to_string(),
        o => panic!("{o:?}"),
    };
    steps[0].confirmation_token = Some(tok.clone());
    let b = g.batch(&steps, true).unwrap();
    assert!(b.ok);
    assert_eq!(g.inner().executed, ["job.start"]);
    // The token was spent.
    assert!(g.batch(&steps, true).is_err());
    assert_eq!(g.inner().executed.len(), 1);
}

#[test]
fn the_same_token_cannot_cover_two_steps() {
    let mut g = gate(allow_all());
    let tok = confirmed_token(&mut g, "job.start", json!({}));
    let step = BatchStep { command: "job.start".into(), params: json!({}), confirmation_token: Some(tok) };
    let e = g.batch(&[step.clone(), step], true).unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationInvalid);
    assert!(g.inner().executed.is_empty());
}

#[test]
fn full_confirmation_flow_through_the_mcp_tools() {
    let mut s = mcp(Box::new(gate(allow_all())));
    let r = call(&mut s, "execute", json!({"command": "job.start"}));
    assert!(is_error(&r));
    let t = text(&r);
    assert!(t.contains("[confirmation_required]"), "{t}");
    let pid = t.split('`').find(|p| p.starts_with('c') && p.len() == 9).expect("a pending id in the message").to_string();
    // Not confirmed yet: pending says it is waiting for the user.
    let r = call(&mut s, "pending", json!({"id": pid, "wait_ms": 0}));
    assert!(text(&r).contains("still running") && text(&r).contains("confirm"), "{}", text(&r));
}

#[test]
fn parameters_are_validated_before_the_backend_runs() {
    let mut g = gate(allow_all());
    let e = exec(&mut g, "laser.test_fire", json!({"power": 500})).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidParams);
    assert_eq!(exec(&mut g, "shape.add_rect", json!({"x": 1})).unwrap_err().code, ErrorCode::InvalidParams);
    assert_eq!(exec(&mut g, "file.new", json!([1])).unwrap_err().code, ErrorCode::InvalidParams);
    assert_eq!(exec(&mut g, "file.new", json!({"surprise": true})).unwrap_err().code, ErrorCode::InvalidParams);
    assert_eq!(exec(&mut g, "nope.none", json!({})).unwrap_err().code, ErrorCode::UnknownCommand);
    assert_eq!(exec(&mut g, "edit.disabled_example", json!({})).unwrap_err().code, ErrorCode::Disabled);
    assert!(g.inner().executed.is_empty());
    // Null params mean "no params".
    exec(&mut g, "file.new", Value::Null).unwrap();
}

#[test]
fn the_audit_log_records_every_call() {
    let mut g = gate(McpPolicy::default());
    let _ = exec(&mut g, "machine.jog", json!({"dx": 7, "dy": 3}));
    exec(&mut g, "shape.add_rect", json!({"x": 1, "y": 2, "width": 3, "height": 4})).unwrap();
    let _ = exec(&mut g, "nope.none", json!({}));
    let _ = g.inspect("design.shapes", 0);
    let _ = g.screenshot(None);
    let _ = g.measure(json!({}));
    let ev = g.events(0).unwrap();
    let audit: Vec<_> = ev.events.iter().filter(|e| e.kind == "audit").collect();
    assert!(audit.len() >= 6, "{audit:?}");
    let jog = audit.iter().find(|e| e.command == "machine.jog").unwrap();
    assert_eq!(jog.outcome, "denied");
    assert_eq!(jog.tool, "execute");
    assert_eq!(jog.args_hash.len(), 16);
    assert!(jog.ts_ms > 1_600_000_000_000);
    let add = audit.iter().find(|e| e.command == "shape.add_rect").unwrap();
    assert_eq!(add.outcome, "ok");
    assert_ne!(add.args_hash, jog.args_hash);
    assert!(audit.iter().any(|e| e.command == "nope.none" && e.outcome == "unknowncommand"));
    assert!(audit.iter().any(|e| e.tool == "screenshot") && audit.iter().any(|e| e.tool == "inspect") && audit.iter().any(|e| e.tool == "measure"));
    // The same arguments hash the same.
    assert_eq!(darkburn_mcp::args_hash(&json!({"a": 1})), darkburn_mcp::args_hash(&json!({"a": 1})));
    assert_ne!(darkburn_mcp::args_hash(&json!({"a": 1})), darkburn_mcp::args_hash(&json!({"a": 2})));
    // Sequence numbers are strictly increasing and `since` pages.
    assert!(ev.events.windows(2).all(|w| w[0].seq < w[1].seq));
    let none = g.events(ev.next).unwrap();
    assert!(none.events.is_empty());
    // The app's own events are merged into the same stream.
    assert!(ev.events.iter().any(|e| e.kind == "command" && e.command == "shape.add_rect"));
}

#[test]
fn confirmations_and_batches_are_audited() {
    let mut g = gate(allow_all());
    let _ = exec(&mut g, "job.start", json!({}));
    let _ = g.batch(
        &[BatchStep { command: "shape.add_rect".into(), params: json!({"x": 0, "y": 0, "width": 1, "height": 1}), confirmation_token: None }],
        true,
    );
    let ev = g.events(0).unwrap().events;
    assert!(ev.iter().any(|e| e.kind == "confirmation" && e.outcome == "requested"));
    assert!(ev.iter().any(|e| e.kind == "audit" && e.tool == "batch" && e.command == "shape.add_rect" && e.outcome == "ok"));
    assert!(ev.iter().any(|e| e.kind == "audit" && e.command == "job.start" && e.outcome == "confirmationrequired"));
}

#[test]
fn the_agent_can_read_the_policy_but_not_open_dialogs_itself() {
    let mut g = gate(McpPolicy::default());
    let v = g.inspect("mcp.policy", 0).unwrap();
    assert_eq!(v["policy"]["allow_hardware"], false);
    assert_eq!(g.request_confirmation("x", "job.start", "s").unwrap_err().code, ErrorCode::Denied);
    // `request_confirmation` is not a control method either.
    let r = darkburn_mcp::control::dispatch(&mut g, "request_confirmation", json!({}));
    assert_eq!(r.unwrap_err().code, ErrorCode::NotSupported);
}

#[test]
fn control_methods_cannot_skip_the_gate() {
    // The control server dispatches into the gate: a hardware command over the wire is denied.
    let mut g = gate(McpPolicy::default());
    let r = darkburn_mcp::control::dispatch(&mut g, "execute", json!({"id": "machine.jog", "args": {"dx": 1, "dy": 1}, "tool": "execute"}));
    assert_eq!(r.unwrap_err().code, ErrorCode::Denied);
    let r = darkburn_mcp::control::dispatch(&mut g, "batch", json!({"steps": [{"command": "machine.jog", "params": {"dx": 1, "dy": 1}}]}));
    assert_eq!(r.unwrap_err().code, ErrorCode::Denied);
    assert!(g.inner().executed.is_empty());
}
