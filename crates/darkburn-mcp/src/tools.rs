//! The MCP tools: their JSON schemas and how each one runs on a [`Backend`].
//!
//! The tools are thin: every action goes through [`Backend::execute_ctx`] (so through the safety
//! gate), every query through [`Backend::inspect`] and friends.

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solidcraft_mcp_server::{Effect, image_result, object, string, text_result, tool};

use crate::backend::Backend;
use crate::server::{Info, Resource, Server, Tools};
use crate::types::*;

pub const AGENT_GUIDE: &str = include_str!("../skill/darkburn-agent-guide.md");
pub const CHECKLIST: &str = include_str!("../../../docs/PARITY-CHECKLIST.md");

/// What this server says about itself.
pub const INFO: Info = Info { name: "darkburn-mcp", title: "DarkBurn", version: env!("CARGO_PKG_VERSION"), max_line: 64 << 20 };

const INSTRUCTIONS: &str = "DarkBurn is a laser cutter / engraver program, and these tools drive it: the user watches the design change in the window. \
Millimetres, Y up. Every function of the program is a command: find it with `list_commands {\"filter\": word}` (never unfiltered), \
read its schema with `describe_command`, run it with `execute` or several with `batch`. Call `skill_reference` first for command ids, \
addressing (shapes by stable id, layers by index or name), worked examples and traps. See what is there with `inspect_design`, \
`inspect_ui`, `measure`, `screenshot`. \
SAFETY: commands that move axes, fire the laser, send jobs or touch a machine are denied until the user turns on \"Allow AI agents to control machines\" \
in DarkBurn's settings; emergency stop, pause and stop always work; starting a job or firing needs the user to click Confirm (the error explains the \
token flow). Never try to get around a denial: tell the user. `preview_job` shows the toolpath or G-code without sending anything. \
A reply starting `still running:` was not cancelled: use `pending`.";

/// How long to wait between polls of a long operation (tests shorten it).
const POLL: Duration = Duration::from_millis(100);
const MAX_WAIT_MS: u64 = 600_000;
const DEFAULT_WAIT_MS: u64 = 10_000;
const MAX_STEPS: usize = 10_000;

pub struct DarkburnTools {
    backend: Box<dyn Backend>,
    poll: Duration,
}

/// An MCP server over `backend` (the stdio loop is `Server::serve`).
pub fn server(backend: Box<dyn Backend>) -> Server<DarkburnTools> {
    Server::new(INFO, DarkburnTools::new(backend))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn ok(v: &Value) -> Value {
    text_result(pretty(v), false)
}

fn fail(e: &CommandError) -> Value {
    let tag = serde_json::to_value(e.code).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    if e.code == ErrorCode::StillRunning { text_result(e.message.clone(), true) } else { text_result(format!("[{tag}] {}", e.message), true) }
}

fn done<T: serde::Serialize>(r: Result<T, CommandError>) -> Value {
    match r {
        Ok(v) => ok(&serde_json::to_value(v).unwrap_or(Value::Null)),
        Err(e) => fail(&e),
    }
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

/// A selector the agent gave, relative to `root`. Accepts `shapes.s1` and `design.shapes.s1`.
fn selector(root: &str, path: Option<&str>) -> Result<String, String> {
    let p = path.unwrap_or("").trim().trim_matches('.');
    if p.is_empty() || p == root {
        return Ok(root.to_string());
    }
    if p.len() > 256 || p.chars().any(char::is_control) || p.split('.').any(str::is_empty) {
        return Err(format!(
            "bad path `{}`: dotted names such as `shapes.s12` or `layers.Cut`, at most 256 characters, no empty segments",
            p.chars().take(60).collect::<String>()
        ));
    }
    Ok(if p.starts_with(&format!("{root}.")) { p.to_string() } else { format!("{root}.{p}") })
}

impl DarkburnTools {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self { backend, poll: POLL }
    }
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }
    pub fn backend(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }

    /// Poll a pending id until it finishes or `wait_ms` is over.
    fn wait(&mut self, id: &str, wait_ms: u64) -> Result<PendingState, CommandError> {
        let start = Instant::now();
        let limit = Duration::from_millis(wait_ms.min(MAX_WAIT_MS));
        loop {
            let st = self.backend.pending(id)?;
            if !matches!(st, PendingState::Running { .. }) || start.elapsed() >= limit {
                return Ok(st);
            }
            std::thread::sleep(self.poll.min(limit.saturating_sub(start.elapsed())).max(Duration::from_millis(1)));
        }
    }

    fn finish_pending(&mut self, id: &str, wait_ms: u64) -> Value {
        match self.wait(id, wait_ms) {
            Err(e) => fail(&e),
            Ok(PendingState::Unknown) => text_result(
                format!("no pending operation `{id}`: it never existed or its result was already delivered; `running` lists what is in progress"),
                true,
            ),
            Ok(PendingState::Running { what, running_s }) => {
                text_result(format!("still running: {what} ({running_s} s); it was not cancelled, call `pending` with id `{id}` again"), true)
            }
            Ok(PendingState::Done { what, result }) => match result {
                Ok(v) => ok(&json!({"ok": true, "done": what, "result": v})),
                Err(e) => fail(&e),
            },
        }
    }

    fn run(&mut self, tool_name: &str, command: &str, params: Value, token: Option<String>, wait_ms: u64) -> Value {
        let ctx = CallContext { tool: tool_name.into(), confirmation_token: token };
        match self.backend.execute_ctx(command, params, &ctx) {
            Ok(v) => ok(&json!({"ok": true, "result": v})),
            Err(e) if e.code == ErrorCode::StillRunning => match e.pending_id.clone() {
                Some(id) => self.finish_pending(&id, wait_ms),
                None => fail(&e),
            },
            Err(e) => fail(&e),
        }
    }

    fn call_inner(&mut self, name: &str, a: &Value) -> Value {
        let wait_ms = a.get("wait_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_WAIT_MS);
        let token = s(a, "confirmation_token").map(str::to_owned);
        let params = |k: &str| a.get(k).cloned().filter(|v| !v.is_null()).unwrap_or_else(|| json!({}));
        match name {
            "list_commands" => {
                let q = ListQuery {
                    filter: s(a, "filter").map(str::to_owned),
                    prefix: s(a, "prefix").map(str::to_owned),
                    panel: s(a, "panel").map(str::to_owned),
                    compact: a.get("compact").and_then(Value::as_bool).unwrap_or(false),
                    limit: Some(a.get("limit").and_then(Value::as_u64).map_or(100, |l| l as usize)),
                    offset: a.get("offset").and_then(Value::as_u64).map(|o| o as usize),
                };
                done(self.backend.list_commands(&q))
            }
            "describe_command" => {
                let id = s(a, "command").unwrap_or("");
                match self.backend.describe_command(id) {
                    Ok(Some(c)) => ok(&serde_json::to_value(c).unwrap_or(Value::Null)),
                    Ok(None) => {
                        text_result(format!("[unknown_command] unknown command `{id}`; use list_commands with a filter to find its id"), true)
                    }
                    Err(e) => fail(&e),
                }
            }
            "execute" => self.run("execute", s(a, "command").unwrap_or(""), params("params"), token, wait_ms),
            "batch" => {
                let steps: Vec<BatchStep> = match serde_json::from_value(a.get("steps").cloned().unwrap_or(Value::Null)) {
                    Ok(v) => v,
                    Err(e) => {
                        return text_result(
                            format!("[invalid_params] batch: bad `steps`: {e}. Each step is {{command, params?, confirmation_token?}}"),
                            true,
                        );
                    }
                };
                if steps.is_empty() || steps.len() > MAX_STEPS {
                    return text_result(format!("[invalid_params] batch: `steps` must hold 1..={MAX_STEPS} steps, got {}", steps.len()), true);
                }
                let all = a.get("all_or_nothing").and_then(Value::as_bool).unwrap_or(true);
                match self.backend.batch(&steps, all) {
                    Ok(b) => {
                        let v = serde_json::to_value(&b).unwrap_or(Value::Null);
                        if b.ok { ok(&v) } else { text_result(pretty(&v), true) }
                    }
                    Err(e) => fail(&e),
                }
            }
            "inspect_design" | "inspect_ui" => {
                let root = if name == "inspect_design" { "design" } else { "ui" };
                let depth = a.get("depth").and_then(Value::as_u64).map_or(0, |d| d.min(64) as u32);
                match selector(root, s(a, "path")) {
                    Ok(p) => done(self.backend.inspect(&p, depth)),
                    Err(e) => text_result(format!("[invalid_params] {e}"), true),
                }
            }
            "get_setting" => match self.backend.inspect(&format!("settings.{}", s(a, "key").unwrap_or("")), 0) {
                Ok(v) => ok(&v),
                Err(e) => fail(&e),
            },
            "list_settings" => match self.backend.inspect("settings", 2) {
                Ok(Value::Array(items)) => {
                    let f = s(a, "filter").map(str::to_lowercase);
                    let kept: Vec<Value> = items
                        .into_iter()
                        .filter(|i| f.as_ref().is_none_or(|f| i["key"].as_str().is_some_and(|k| k.to_lowercase().contains(f.as_str()))))
                        .collect();
                    ok(&json!({"settings": kept}))
                }
                Ok(v) => ok(&v),
                Err(e) => fail(&e),
            },
            "set_setting" => self.run("set_setting", "settings.set", json!({"key": s(a, "key"), "value": a.get("value")}), None, wait_ms),
            "new_design" => self.run("new_design", "file.new", json!({}), None, wait_ms),
            "open" => self.run("open", "file.open", json!({"path": s(a, "path")}), None, wait_ms),
            "save" => match s(a, "path") {
                Some(p) => self.run("save", "file.save_as", json!({"path": p}), None, wait_ms),
                None => self.run("save", "file.save", json!({}), None, wait_ms),
            },
            "import" => {
                let mut p: Map<String, Value> = a.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
                p.insert("path".into(), json!(s(a, "path")));
                self.run("import", "file.import", Value::Object(p), None, wait_ms)
            }
            "export" => {
                let mut p = Map::new();
                p.insert("path".into(), json!(s(a, "path")));
                if let Some(f) = s(a, "format") {
                    p.insert("format".into(), json!(f));
                }
                self.run("export", "file.export", Value::Object(p), None, wait_ms)
            }
            "undo" | "redo" => {
                let mut p = Map::new();
                if let Some(c) = a.get("count").and_then(Value::as_u64) {
                    p.insert("count".into(), json!(c));
                }
                self.run(name, if name == "undo" { "edit.undo" } else { "edit.redo" }, Value::Object(p), None, wait_ms)
            }
            "screenshot" => {
                let region = a.get("region").cloned().filter(|v| !v.is_null()).and_then(|v| serde_json::from_value::<Region>(v).ok());
                if a.get("region").is_some_and(|v| !v.is_null()) && region.is_none() {
                    return text_result("[invalid_params] screenshot: `region` is {x, y, width, height} in logical pixels", true);
                }
                match self.backend.screenshot(region) {
                    Ok(png) if png.len() > 32 << 20 => text_result("[failed] the screenshot is larger than 32 MiB: ask for a smaller region", true),
                    Ok(png) => image_result(
                        &png,
                        "image/png",
                        format!(
                            "DarkBurn window{}",
                            region.map_or(String::new(), |r| format!(", region x={} y={} w={} h={}", r.x, r.y, r.width, r.height))
                        ),
                    ),
                    Err(e) => fail(&e),
                }
            }
            "measure" => done(self.backend.measure(strip_nulls(a))),
            "preview_job" => done(self.backend.preview_job(strip_nulls(a))),
            "machine_status" => done(self.backend.inspect("machine.status", 0)),
            "list_devices" => done(self.backend.inspect("machine.devices", 0)),
            "pending" => match s(a, "id") {
                Some(id) => self.finish_pending(id, a.get("wait_ms").and_then(Value::as_u64).unwrap_or(DEFAULT_WAIT_MS)),
                None => match self.backend.running() {
                    Ok(v) if v.is_empty() => ok(&json!({"pending": null, "note": "nothing is running"})),
                    Ok(v) => ok(&json!({"pending": v})),
                    Err(e) => fail(&e),
                },
            },
            "running" => done(self.backend.running().map(|v| json!({"running": v}))),
            "events" => {
                let since = a.get("since").and_then(Value::as_u64).unwrap_or(0);
                let limit = a.get("limit").and_then(Value::as_u64).map_or(200, |l| l as usize);
                match self.backend.events(since) {
                    Ok(mut p) => {
                        if p.events.len() > limit {
                            let cut = p.events.len() - limit;
                            p.events.drain(..cut);
                        }
                        ok(&serde_json::to_value(p).unwrap_or(Value::Null))
                    }
                    Err(e) => fail(&e),
                }
            }
            "skill_reference" => text_result(AGENT_GUIDE, false),
            "crash_reports" => done(self.backend.crash_reports(a.get("limit").and_then(Value::as_u64).map_or(20, |l| l as usize))),
            _ => text_result(format!("unknown tool `{name}`"), true),
        }
    }
}

fn strip_nulls(a: &Value) -> Value {
    match a {
        Value::Object(m) => Value::Object(m.iter().filter(|(k, v)| !v.is_null() && *k != "wait_ms").map(|(k, v)| (k.clone(), v.clone())).collect()),
        _ => json!({}),
    }
}

impl Tools for DarkburnTools {
    fn instructions(&self) -> String {
        format!("{INSTRUCTIONS} Backend: {}.", self.backend.describe())
    }

    fn list(&mut self) -> Vec<Value> {
        tool_definitions()
    }

    fn call(&mut self, name: &str, args: &Value) -> Value {
        self.call_inner(name, args)
    }

    fn resources(&self) -> Vec<Value> {
        vec![
            json!({"uri": "darkburn://checklist", "name": "Parity checklist", "description": "Every feature DarkBurn must have, with done / partly / not started marks", "mimeType": "text/markdown"}),
            json!({"uri": "darkburn://agent-guide", "name": "Agent guide", "description": "Command ids, addressing, units, worked examples and traps", "mimeType": "text/markdown"}),
            json!({"uri": "darkburn://commands", "name": "Command catalog", "description": "Compact list of every command (id, title, group, enabled, hardware)", "mimeType": "application/json"}),
        ]
    }

    fn read_resource(&mut self, uri: &str) -> Option<Result<Resource, String>> {
        match uri {
            "darkburn://checklist" => Some(Ok(Resource { mime: "text/markdown", text: CHECKLIST.to_string() })),
            "darkburn://agent-guide" => Some(Ok(Resource { mime: "text/markdown", text: AGENT_GUIDE.to_string() })),
            "darkburn://commands" => Some(
                self.backend
                    .list_commands(&ListQuery { compact: true, ..Default::default() })
                    .map(|p| Resource { mime: "application/json", text: pretty(&serde_json::to_value(p).unwrap_or(Value::Null)) })
                    .map_err(|e| e.message),
            ),
            _ => None,
        }
    }
}

fn wait_prop() -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "How long to wait for a long operation, ms (default 10000). Past it the reply says `still running:`; the operation is not cancelled"})
}

/// Every tool with its JSON schema.
pub fn tool_definitions() -> Vec<Value> {
    let path = || json!({"type": "string", "description": "Path of the file, absolute"});
    let region = json!({"type": "object", "description": "Region in logical pixels", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "width": {"type": "number", "exclusiveMinimum": 0}, "height": {"type": "number", "exclusiveMinimum": 0}}, "required": ["x", "y", "width", "height"], "additionalProperties": false});
    let step = json!({"type": "object", "properties": {"command": string("Command id"), "params": {"type": "object", "description": "Parameters per the command's schema"}, "confirmation_token": string("One-time token for a command that needs the user's confirmation")}, "required": ["command"], "additionalProperties": false});
    vec![
        tool(
            "list_commands",
            "List commands",
            "The command catalog: id, title, group, menu path, shortcut, enabled now, hardware flag, parameter schema, checklist ids. Every menu item, button, tool action, panel and dialog action and setting is a command. Filter it: `filter` (case-insensitive substring over id, title, group, menu path), `prefix` (start of the id), `panel` (a whole group), `compact` (id, title, enabled, hardware only), `limit` (default 100) and `offset`. The reply is {commands, total, next_offset}.",
            object(
                json!({"filter": string("Substring over id, title, group, menu path"), "prefix": string("Start of the id, e.g. `edit.`"), "panel": string("Whole group / panel name"), "compact": {"type": "boolean"}, "limit": {"type": "integer", "minimum": 1, "maximum": 5000}, "offset": {"type": "integer", "minimum": 0}}),
                &[],
            ),
            Effect::ReadOnly,
        ),
        tool(
            "describe_command",
            "Describe a command",
            "One command with its parameter and result JSON Schemas, whether it is enabled, and whether it is a hardware command.",
            object(json!({"command": string("Command id, e.g. `file.save_as`")}), &["command"]),
            Effect::ReadOnly,
        ),
        tool(
            "execute",
            "Run a command",
            "Run one command by id with `params` (checked against the command's schema). Anything the GUI can do is a command. Hardware commands are denied unless the user allowed AI control of machines; starting a job or firing needs the user's confirmation (`confirmation_token`, see the error). A long operation waits up to `wait_ms`, then replies `still running:` (not cancelled; use `pending`).",
            object(
                json!({"command": string("Command id"), "params": {"type": "object", "description": "Parameters per the command's schema"}, "confirmation_token": string("One-time token the user's Confirm click produced; from `pending`"), "wait_ms": wait_prop()}),
                &["command"],
            ),
            Effect::Files,
        ),
        tool(
            "batch",
            "Run commands in order",
            "Run steps in order. Every step is checked (existence, parameters, safety) before any runs, so one denied step rejects the batch. With `all_or_nothing` (default true) a failing step undoes the earlier ones when the app can; the reply says `rolled_back`. The reply lists each step's result and names the failing step.",
            object(
                json!({"steps": {"type": "array", "items": step, "minItems": 1, "maxItems": MAX_STEPS}, "all_or_nothing": {"type": "boolean"}}),
                &["steps"],
            ),
            Effect::Files,
        ),
        tool(
            "inspect_design",
            "Inspect the design",
            "The open design: shapes (stable ids), layers (index or name), selection, transforms, cut settings. `path` is dotted and relative to the design: `shapes`, `shapes.s12`, `layers`, `layers.Cut`, `layers.0`, `selection`, `cut_settings`. `depth` limits nesting.",
            object(
                json!({"path": string("Dotted selector, e.g. `shapes.s12`; empty is the whole design"), "depth": {"type": "integer", "minimum": 0, "maximum": 64}}),
                &[],
            ),
            Effect::ReadOnly,
        ),
        tool(
            "inspect_ui",
            "Inspect the UI",
            "The app's interface: panels, open dialogs (and their commands), focus, tool in use, undo stack. `path` is dotted relative to the UI root, e.g. `dialogs`, `panels.cuts`.",
            object(json!({"path": string("Dotted selector; empty is the whole UI"), "depth": {"type": "integer", "minimum": 0, "maximum": 64}}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "get_setting",
            "Read a setting",
            "The value of one setting, by key (see list_settings).",
            object(json!({"key": string("Setting key, e.g. `grid.spacing`")}), &["key"]),
            Effect::ReadOnly,
        ),
        tool(
            "set_setting",
            "Change a setting",
            "Change one setting (runs `settings.set`). Settings that write to a machine are hardware commands, not this.",
            object(json!({"key": string("Setting key"), "value": {"description": "New value, of the setting's type"}}), &["key", "value"]),
            Effect::Edits,
        ),
        tool(
            "list_settings",
            "List settings",
            "All settings with key, value, type and allowed values. `filter` is a case-insensitive substring of the key.",
            object(json!({"filter": string("Substring of the key")}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "new_design",
            "New design",
            "Start an empty design (`file.new`). Unsaved changes of the current design are lost.",
            object(json!({}), &[]),
            Effect::Replaces,
        ),
        tool(
            "open",
            "Open a file",
            "Open a DarkBurn project (`file.open`). Replaces the current design.",
            object(json!({"path": path()}), &["path"]),
            Effect::Replaces,
        ),
        tool(
            "save",
            "Save",
            "Save the design; with `path` it is Save As (`file.save_as`). Needs the user's permission to write files.",
            object(json!({"path": path()}), &[]),
            Effect::Files,
        ),
        tool(
            "import",
            "Import a file",
            "Import SVG, DXF, images etc. into the design (`file.import`); `options` are that command's parameters.",
            object(
                json!({"path": path(), "options": {"type": "object", "description": "Extra parameters of file.import (describe_command)"}}),
                &["path"],
            ),
            Effect::Edits,
        ),
        tool(
            "export",
            "Export",
            "Export the design or a machine file (`file.export`). Needs the user's permission to write files.",
            object(json!({"path": path(), "format": string("Format, per file.export's schema")}), &["path"]),
            Effect::Files,
        ),
        tool("undo", "Undo", "Undo `count` steps (`edit.undo`).", object(json!({"count": {"type": "integer", "minimum": 1}}), &[]), Effect::Edits),
        tool("redo", "Redo", "Redo `count` steps (`edit.redo`).", object(json!({"count": {"type": "integer", "minimum": 1}}), &[]), Effect::Edits),
        tool(
            "screenshot",
            "Screenshot",
            "PNG of the app window, or of `region` (logical pixels).",
            object(json!({"region": region}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "measure",
            "Measure",
            "Bounding boxes, lengths, areas and the estimated job time of the design, the selection (`target: \"selection\"`) or shape ids (`target: [\"s1\",\"s2\"]`). `what` limits the quantities.",
            object(
                json!({"target": {"type": ["string", "array"], "items": {"type": "string"}, "description": "`design` (default), `selection`, a layer name, or a list of shape ids"}, "what": {"type": "array", "items": {"type": "string", "enum": ["bbox", "length", "area", "time"]}}}),
                &[],
            ),
            Effect::ReadOnly,
        ),
        tool(
            "preview_job",
            "Preview the job",
            "What the job would do, WITHOUT sending it anywhere: `format: \"summary\"` (moves, passes, per-layer time and distance, bounds) or `\"gcode\"` (the text, capped by `max_lines`).",
            object(
                json!({"format": {"type": "string", "enum": ["summary", "gcode"]}, "max_lines": {"type": "integer", "minimum": 1}, "layers": {"type": "array", "items": {"type": ["string", "integer"]}}}),
                &[],
            ),
            Effect::ReadOnly,
        ),
        tool(
            "machine_status",
            "Machine status",
            "Connection state, machine state, position, alarms of the connected machine (read only).",
            object(json!({}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "list_devices",
            "List devices",
            "Known and detected devices. Only lists; never opens a port or connects.",
            object(json!({}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "pending",
            "Wait for a pending operation",
            "The result of a long operation or of a confirmation, by `id` (waits up to `wait_ms`). A confirmation id returns `confirmation_token` once the user clicked Confirm. Without `id` it lists what is pending.",
            object(json!({"id": string("Pending id"), "wait_ms": wait_prop()}), &[]),
            Effect::ReadOnly,
        ),
        tool("running", "Running operations", "Long operations and confirmations in progress.", object(json!({}), &[]), Effect::ReadOnly),
        tool(
            "events",
            "Event log",
            "What happened lately: command results, dialogs opened, errors, machine events, and the audit log of MCP calls (timestamp, tool, command, args hash, outcome). Pass the previous `next` as `since`.",
            object(json!({"since": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 1, "maximum": 10000}}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "skill_reference",
            "Agent guide",
            "The agent guide in Markdown: command ids, addressing, units, safety, worked examples and traps. Read it first.",
            object(json!({}), &[]),
            Effect::ReadOnly,
        ),
        tool(
            "crash_reports",
            "Crash reports",
            "Crash reports DarkBurn wrote, newest first.",
            object(json!({"limit": {"type": "integer", "minimum": 1, "maximum": 200}}), &[]),
            Effect::ReadOnly,
        ),
    ]
}
