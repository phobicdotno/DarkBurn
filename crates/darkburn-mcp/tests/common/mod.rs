#![allow(dead_code)]
//! Shared helpers for the integration tests.

use std::time::Duration;

use darkburn_mcp::server::Server;
use darkburn_mcp::tools::{DarkburnTools, INFO};
use darkburn_mcp::{Backend, Gate, MockBackend, PolicyHandle};
use serde_json::{Value, json};

pub fn mcp(backend: Box<dyn Backend>) -> Server<DarkburnTools> {
    Server::new(INFO, DarkburnTools::new(backend).with_poll(Duration::from_millis(1)))
}

/// A server over a gated mock with the default (safe) policy.
pub fn default_server() -> Server<DarkburnTools> {
    mcp(Box::new(Gate::new(MockBackend::new(), PolicyHandle::default())))
}

/// One JSON-RPC request, the parsed reply.
pub fn rpc(server: &mut Server<DarkburnTools>, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let reply = server.handle_line(&line).expect("a reply");
    serde_json::from_str(&reply).expect("reply is JSON")
}

/// `tools/call`: the result object (an `isError` result is still a result).
pub fn call(server: &mut Server<DarkburnTools>, tool: &str, args: Value) -> Value {
    let r = rpc(server, "tools/call", json!({"name": tool, "arguments": args}));
    assert!(r.get("error").is_none(), "{tool}: JSON-RPC error {r}");
    r["result"].clone()
}

pub fn text(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

pub fn is_error(result: &Value) -> bool {
    result["isError"] == true
}

/// The reply text parsed as JSON.
pub fn json_of(result: &Value) -> Value {
    serde_json::from_str(&text(result)).unwrap_or_else(|e| panic!("not JSON ({e}): {}", text(result)))
}

pub fn gate(policy: darkburn_mcp::McpPolicy) -> Gate<MockBackend> {
    Gate::new(MockBackend::new(), PolicyHandle::new(policy))
}

pub fn allow_all() -> darkburn_mcp::McpPolicy {
    darkburn_mcp::McpPolicy { allow_hardware: true, allow_file_write: true, require_confirmation_for_job_start: true, allow_gweike: false }
}
