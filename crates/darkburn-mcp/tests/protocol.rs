mod common;

use common::*;
use darkburn_mcp::server::{Info, Server};
use darkburn_mcp::tools::{DarkburnTools, INFO, tool_definitions};
use darkburn_mcp::{Gate, MockBackend, PolicyHandle};
use serde_json::{Value, json};

const EXPECTED_TOOLS: &[&str] = &[
    "list_commands",
    "describe_command",
    "execute",
    "batch",
    "inspect_design",
    "inspect_ui",
    "get_setting",
    "set_setting",
    "list_settings",
    "new_design",
    "open",
    "save",
    "import",
    "export",
    "undo",
    "redo",
    "screenshot",
    "measure",
    "preview_job",
    "machine_status",
    "list_devices",
    "pending",
    "running",
    "skill_reference",
    "crash_reports",
    "events",
];

#[test]
fn initialize_negotiates_and_describes() {
    let mut s = default_server();
    let r = rpc(&mut s, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "darkburn-mcp");
    let ins = r["result"]["instructions"].as_str().unwrap();
    assert!(ins.contains("Allow AI agents to control machines"), "{ins}");
    assert!(ins.contains("skill_reference"));
    // An unknown version is answered with the newest one we speak.
    let r = rpc(&mut s, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(s.is_initialized());
    assert_eq!(rpc(&mut s, "ping", json!({}))["result"], json!({}));
}

#[test]
fn tools_list_has_every_tool_with_valid_schemas() {
    let mut s = default_server();
    let r = rpc(&mut s, "tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in EXPECTED_TOOLS {
        assert!(names.contains(want), "missing tool {want}");
    }
    assert_eq!(names.len(), EXPECTED_TOOLS.len(), "unexpected extra tools: {names:?}");
    for t in tools {
        let name = t["name"].as_str().unwrap();
        assert!(name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{name} is not snake_case");
        assert!(!t["description"].as_str().unwrap().is_empty(), "{name} has no description");
        assert_eq!(t["inputSchema"]["type"], "object", "{name}");
        assert_eq!(t["inputSchema"]["additionalProperties"], false, "{name}: unknown arguments must be refused");
        jsonschema::validator_for(&t["inputSchema"]).unwrap_or_else(|e| panic!("{name}: invalid schema: {e}"));
        assert!(t["annotations"]["readOnlyHint"].is_boolean());
    }
    assert_eq!(tool_definitions().len(), EXPECTED_TOOLS.len());
}

#[test]
fn read_only_tools_are_annotated_read_only() {
    let defs = tool_definitions();
    for n in [
        "list_commands",
        "describe_command",
        "inspect_design",
        "inspect_ui",
        "screenshot",
        "measure",
        "preview_job",
        "machine_status",
        "list_devices",
        "events",
    ] {
        let t = defs.iter().find(|t| t["name"] == n).unwrap();
        assert_eq!(t["annotations"]["readOnlyHint"], true, "{n}");
    }
    for n in ["execute", "batch", "save", "export", "open"] {
        let t = defs.iter().find(|t| t["name"] == n).unwrap();
        assert_eq!(t["annotations"]["readOnlyHint"], false, "{n}");
        assert_eq!(t["annotations"]["destructiveHint"], true, "{n}");
    }
}

#[test]
fn list_and_describe_commands() {
    let mut s = default_server();
    let r = json_of(&call(&mut s, "list_commands", json!({"filter": "shape"})));
    let ids: Vec<&str> = r["commands"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["shape.add_rect", "shape.delete", "shape.move"]);
    assert_eq!(r["total"], 3);
    let r = json_of(&call(&mut s, "list_commands", json!({"prefix": "machine.", "compact": true})));
    let c = &r["commands"][0];
    assert!(c.get("params_schema").is_none());
    assert!(r["commands"].as_array().unwrap().iter().any(|c| c["id"] == "machine.jog" && c["hardware"] == true));
    // Paging.
    let r = json_of(&call(&mut s, "list_commands", json!({"limit": 5, "compact": true})));
    assert_eq!(r["commands"].as_array().unwrap().len(), 5);
    assert_eq!(r["next_offset"], 5);
    assert!(r["total"].as_u64().unwrap() > 5);
    let r = json_of(&call(&mut s, "list_commands", json!({"panel": "laser"})));
    assert!(r["commands"].as_array().unwrap().iter().all(|c| c["group"] == "Laser"));

    let d = json_of(&call(&mut s, "describe_command", json!({"command": "shape.add_rect"})));
    assert_eq!(d["id"], "shape.add_rect");
    assert_eq!(d["params_schema"]["required"], json!(["x", "y", "width", "height"]));
    assert_eq!(d["hardware"], false);
    let e = call(&mut s, "describe_command", json!({"command": "nope.nothing"}));
    assert!(is_error(&e));
    assert!(text(&e).contains("unknown command"), "{}", text(&e));
}

#[test]
fn execute_and_inspect_use_stable_ids() {
    let mut s = default_server();
    let r = json_of(&call(
        &mut s,
        "execute",
        json!({"command": "shape.add_rect", "params": {"x": 1, "y": 2, "width": 10, "height": 5, "layer": "Engrave"}}),
    ));
    assert_eq!(r["ok"], true);
    let id = r["result"]["id"].as_str().unwrap().to_string();
    let a = json_of(&call(&mut s, "inspect_design", json!({"path": format!("shapes.{id}")})));
    let b = json_of(&call(&mut s, "inspect_design", json!({"path": format!("design.shapes.{id}")})));
    assert_eq!(a, b);
    assert_eq!(a["layer"], 1);
    let l = json_of(&call(&mut s, "inspect_design", json!({"path": "layers.Engrave"})));
    assert_eq!(l["shapes"], json!([id]));
    let l0 = json_of(&call(&mut s, "inspect_design", json!({"path": "layers.0"})));
    assert_eq!(l0["name"], "Cut");
    let all = json_of(&call(&mut s, "inspect_design", json!({})));
    assert_eq!(all["shapes"].as_array().unwrap().len(), 1);
    let bad = call(&mut s, "inspect_design", json!({"path": "shapes..x"}));
    assert!(is_error(&bad));
    let ui = json_of(&call(&mut s, "inspect_ui", json!({})));
    assert!(ui["panels"].is_array());
}

#[test]
fn undo_redo_tools() {
    let mut s = default_server();
    call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 1, "height": 1}}));
    let u = call(&mut s, "undo", json!({}));
    assert!(!is_error(&u), "{}", text(&u));
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))), json!([]));
    call(&mut s, "redo", json!({"count": 1}));
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))).as_array().unwrap().len(), 1);
}

#[test]
fn jsonrpc_errors() {
    let mut s = default_server();
    assert_eq!(rpc(&mut s, "no/such/method", json!({}))["error"]["code"], -32601);
    let r: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32700);
    let r = rpc(&mut s, "tools/call", json!({"arguments": {}}));
    assert_eq!(r["error"]["code"], -32602);
    let r = rpc(&mut s, "resources/read", json!({"uri": "darkburn://nothing"}));
    assert_eq!(r["error"]["code"], -32002);
}

#[test]
fn invalid_arguments_are_protocol_errors_and_never_run() {
    let mut s = default_server();
    for (tool, args, needle) in [
        ("nonexistent_tool", json!({}), "unknown tool"),
        ("execute", json!({}), "missing required argument `command`"),
        ("execute", json!({"command": 5}), "must be of type"),
        ("execute", json!({"command": "file.new", "bogus": 1}), "unknown argument `bogus`"),
        ("batch", json!({"steps": "x"}), "must be of type"),
        ("screenshot", json!({"region": "all"}), "must be of type"),
        ("list_commands", json!({"limit": "ten"}), "must be of type"),
        ("preview_job", json!({"format": "pdf"}), "must be one of"),
        ("set_setting", json!({"key": "units"}), "missing required argument `value`"),
    ] {
        let r = rpc(&mut s, "tools/call", json!({"name": tool, "arguments": args}));
        assert_eq!(r["error"]["code"], -32602, "{tool} {args}: {r}");
        assert!(r["error"]["message"].as_str().unwrap().contains(needle), "{tool}: {}", r["error"]["message"]);
    }
}

#[test]
fn params_are_checked_against_the_command_schema() {
    let mut s = default_server();
    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": -3, "height": 1}}));
    assert!(is_error(&r));
    assert!(text(&r).contains("invalid_params"), "{}", text(&r));
    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": "zero", "y": 0, "width": 1, "height": 1}}));
    assert!(is_error(&r) && text(&r).contains("invalid params"), "{}", text(&r));
    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 1, "height": 1, "colour": "red"}}));
    assert!(is_error(&r), "unknown parameter must be refused");
    let r = call(&mut s, "execute", json!({"command": "no.such.thing"}));
    assert!(is_error(&r) && text(&r).contains("unknown_command"), "{}", text(&r));
    let r = call(&mut s, "execute", json!({"command": "edit.disabled_example"}));
    assert!(is_error(&r) && text(&r).contains("disabled"), "{}", text(&r));
    // Nothing was added by the failed calls.
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))), json!([]));
}

#[test]
fn settings_tools() {
    let mut s = default_server();
    let all = json_of(&call(&mut s, "list_settings", json!({"filter": "grid"})));
    assert_eq!(all["settings"].as_array().unwrap().len(), 1);
    call(&mut s, "set_setting", json!({"key": "grid.spacing", "value": 5}));
    assert_eq!(json_of(&call(&mut s, "get_setting", json!({"key": "grid.spacing"})))["value"], 5);
    let r = call(&mut s, "set_setting", json!({"key": "no.such", "value": 1}));
    assert!(is_error(&r));
    let r = call(&mut s, "get_setting", json!({"key": "no.such"}));
    assert!(is_error(&r));
}

#[test]
fn screenshot_returns_png_image_content() {
    let mut s = default_server();
    let r = call(&mut s, "screenshot", json!({"region": {"x": 0, "y": 0, "width": 10, "height": 10}}));
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["content"][0]["mimeType"], "image/png");
    assert!(r["content"][0]["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    let bad = call(&mut s, "screenshot", json!({"region": {"x": 0}}));
    assert!(is_error(&bad));
}

#[test]
fn measure_preview_and_machine_reads() {
    let mut s = default_server();
    call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 10, "height": 5}}));
    let m = json_of(&call(&mut s, "measure", json!({"target": "design"})));
    assert_eq!(m["area_mm2"], 50.0);
    assert_eq!(m["bbox"]["max"], json!([10.0, 5.0]));
    let p = json_of(&call(&mut s, "preview_job", json!({"format": "gcode"})));
    assert!(p["gcode"].as_str().unwrap().contains("G21"));
    assert_eq!(p["sent"], false);
    assert_eq!(json_of(&call(&mut s, "machine_status", json!({})))["connected"], false);
    assert!(json_of(&call(&mut s, "list_devices", json!({}))).is_array());
    assert_eq!(json_of(&call(&mut s, "crash_reports", json!({}))), json!([]));
}

#[test]
fn skill_reference_and_resources() {
    let mut s = default_server();
    let g = text(&call(&mut s, "skill_reference", json!({})));
    for needle in ["Y is up", "Millimetres", "confirmation_token", "Addressing", "Traps"] {
        assert!(g.contains(needle), "guide lacks {needle}");
    }
    let list = rpc(&mut s, "resources/list", json!({}));
    let uris: Vec<&str> = list["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, ["darkburn://checklist", "darkburn://agent-guide", "darkburn://commands"]);
    let c = rpc(&mut s, "resources/read", json!({"uri": "darkburn://checklist"}));
    assert_eq!(c["result"]["contents"][0]["mimeType"], "text/markdown");
    assert!(c["result"]["contents"][0]["text"].as_str().unwrap().contains("DarkBurn parity checklist"));
    let c = rpc(&mut s, "resources/read", json!({"uri": "darkburn://agent-guide"}));
    assert!(c["result"]["contents"][0]["text"].as_str().unwrap().starts_with("# DarkBurn agent guide"));
    let c = rpc(&mut s, "resources/read", json!({"uri": "darkburn://commands"}));
    assert_eq!(c["result"]["contents"][0]["mimeType"], "application/json");
    let cat: Value = serde_json::from_str(c["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert!(cat["total"].as_u64().unwrap() > 10);
}

#[test]
fn guide_names_only_real_tools() {
    // Every `tool_name {` example in the guide must be a tool we serve.
    let names: Vec<String> = tool_definitions().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for line in darkburn_mcp::tools::AGENT_GUIDE.lines() {
        let t = line.trim();
        if let Some((head, _)) = t.split_once(" {")
            && head.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            && !head.is_empty()
        {
            assert!(names.iter().any(|n| n == head), "guide mentions unknown tool `{head}`");
        }
    }
}

#[test]
fn batch_runs_in_order_and_reports() {
    let mut s = default_server();
    let r = call(
        &mut s,
        "batch",
        json!({"steps": [
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.add_rect", "params": {"x": 5, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.move", "params": {"id": "s1", "dx": 1, "dy": 1}},
        ]}),
    );
    assert!(!is_error(&r), "{}", text(&r));
    let b = json_of(&r);
    assert_eq!(b["ok"], true);
    assert_eq!(b["steps"].as_array().unwrap().len(), 3);
    assert_eq!(b["steps"][1]["result"]["id"], "s2");
}

#[test]
fn batch_all_or_nothing_rolls_back() {
    let mut s = default_server();
    let r = call(
        &mut s,
        "batch",
        json!({"steps": [
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.move", "params": {"id": "s99", "dx": 1, "dy": 1}},
            {"command": "shape.add_rect", "params": {"x": 9, "y": 9, "width": 1, "height": 1}},
        ]}),
    );
    assert!(is_error(&r));
    let b = json_of(&r);
    assert_eq!(b["failed_step"], 1);
    assert_eq!(b["rolled_back"], true);
    assert_eq!(b["skipped"], 1);
    assert!(b["steps"][1]["error"]["message"].as_str().unwrap().contains("s99"));
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))), json!([]), "step 0 must be undone");
}

#[test]
fn batch_without_all_or_nothing_keeps_earlier_steps() {
    let mut s = default_server();
    let r = call(
        &mut s,
        "batch",
        json!({"all_or_nothing": false, "steps": [
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.delete", "params": {"id": "nope"}},
        ]}),
    );
    assert!(is_error(&r));
    assert_eq!(json_of(&r)["rolled_back"], false);
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))).as_array().unwrap().len(), 1);
}

#[test]
fn batch_with_a_backend_that_cannot_roll_back_says_so() {
    let mut mock = MockBackend::new();
    mock.no_rollback = true;
    let mut s = mcp(Box::new(Gate::new(mock, PolicyHandle::default())));
    let r = call(
        &mut s,
        "batch",
        json!({"steps": [
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.delete", "params": {"id": "nope"}},
        ]}),
    );
    let b = json_of(&r);
    assert_eq!(b["rolled_back"], false);
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))).as_array().unwrap().len(), 1);
}

#[test]
fn batch_validates_every_step_before_running_any() {
    let mut s = default_server();
    let r = call(
        &mut s,
        "batch",
        json!({"steps": [
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}},
            {"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": -1, "height": 4}},
        ]}),
    );
    assert!(is_error(&r));
    assert!(text(&r).contains("nothing ran") && text(&r).contains("Step 1"), "{}", text(&r));
    let r = call(
        &mut s,
        "batch",
        json!({"steps": [{"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 4, "height": 4}}, {"command": "bogus.id"}]}),
    );
    assert!(is_error(&r) && text(&r).contains("unknown command"), "{}", text(&r));
    assert_eq!(json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))), json!([]));
    let r = call(&mut s, "batch", json!({"steps": []}));
    assert!(is_error(&r));
    let r = rpc(&mut s, "tools/call", json!({"name": "batch", "arguments": {"steps": [{"command": "x", "extra": 1}]}}));
    assert!(r.get("result").is_some_and(|r| r["isError"] == true) || r.get("error").is_some());
}

#[test]
fn long_operations_report_still_running_then_pending_delivers() {
    let mut s = default_server();
    let r = call(&mut s, "execute", json!({"command": "job.long_task", "wait_ms": 0}));
    assert!(is_error(&r));
    assert!(text(&r).starts_with("still running:"), "{}", text(&r));
    let run = json_of(&call(&mut s, "running", json!({})));
    assert_eq!(run["running"][0]["id"], "p1");
    let r = call(&mut s, "pending", json!({"id": "p1", "wait_ms": 5000}));
    assert!(!is_error(&r), "{}", text(&r));
    assert_eq!(json_of(&r)["result"]["done"], true);
    // Delivered once.
    let r = call(&mut s, "pending", json!({"id": "p1"}));
    assert!(is_error(&r) && text(&r).contains("no pending operation"));
    // A default wait finishes it inside the execute call.
    let r = call(&mut s, "execute", json!({"command": "job.long_task"}));
    assert!(!is_error(&r), "{}", text(&r));
    assert_eq!(json_of(&call(&mut s, "running", json!({})))["running"], json!([]));
}

#[test]
fn large_arguments_pass_through() {
    let mut s = default_server();
    let big = "x".repeat(5 << 20);
    let r = call(&mut s, "set_setting", json!({"key": "units", "value": big}));
    assert!(!is_error(&r), "{}", text(&r));
    let v = json_of(&call(&mut s, "get_setting", json!({"key": "units"})));
    assert_eq!(v["value"].as_str().unwrap().len(), 5 << 20);
    // A batch of many steps.
    let steps: Vec<Value> = (0..2000).map(|i| json!({"command": "shape.add_rect", "params": {"x": i, "y": 0, "width": 1, "height": 1}})).collect();
    let b = json_of(&call(&mut s, "batch", json!({"steps": steps})));
    assert_eq!(b["ok"], true);
    assert_eq!(b["steps"].as_array().unwrap().len(), 2000);
}

#[test]
fn oversized_request_lines_are_refused_and_serving_continues() {
    let tools = DarkburnTools::new(Box::new(Gate::new(MockBackend::new(), PolicyHandle::default())));
    let mut s = Server::new(Info { max_line: 2000, ..INFO }, tools);
    let big = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{{\"pad\":\"{}\"}}}}\n", "a".repeat(10_000));
    let input = format!("{big}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}}\n");
    let mut out = Vec::new();
    s.serve(std::io::Cursor::new(input.into_bytes()), &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32600);
    assert_eq!(lines[1]["id"], 2);
}

#[test]
fn serve_loop_over_a_byte_stream() {
    let mut s = default_server();
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"new_design\",\"arguments\":{}}}\n",
    );
    let mut out = Vec::new();
    s.serve(std::io::Cursor::new(input.as_bytes().to_vec()), &mut out).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
    let second: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["id"], 2);
    assert!(second["result"]["isError"].is_null());
}

#[test]
fn events_tool_returns_the_audit_log() {
    let mut s = default_server();
    call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": 0, "y": 0, "width": 1, "height": 1}}));
    call(&mut s, "execute", json!({"command": "machine.jog", "params": {"dx": 1, "dy": 1}}));
    let ev = json_of(&call(&mut s, "events", json!({})));
    let events = ev["events"].as_array().unwrap();
    let audit: Vec<&Value> = events.iter().filter(|e| e["kind"] == "audit").collect();
    assert!(audit.iter().any(|e| e["command"] == "shape.add_rect" && e["outcome"] == "ok" && e["tool"] == "execute"));
    assert!(audit.iter().any(|e| e["command"] == "machine.jog" && e["outcome"] == "denied"));
    let next = ev["next"].as_u64().unwrap();
    let again = json_of(&call(&mut s, "events", json!({"since": next})));
    assert!(again["events"].as_array().unwrap().is_empty());
}

#[test]
fn concurrent_servers_do_not_share_state() {
    // Each thread has its own server and backend: nothing leaks between them.
    let handles: Vec<_> = (0..6)
        .map(|i| {
            std::thread::spawn(move || {
                let mut s = default_server();
                for k in 0..20 {
                    let r = call(&mut s, "execute", json!({"command": "shape.add_rect", "params": {"x": i, "y": k, "width": 1, "height": 1}}));
                    assert!(!is_error(&r));
                }
                json_of(&call(&mut s, "inspect_design", json!({"path": "shapes"}))).as_array().unwrap().len()
            })
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), 20);
    }
}
