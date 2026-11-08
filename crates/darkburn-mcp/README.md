# darkburn-mcp

The MCP (Model Context Protocol) layer for DarkBurn. An AI agent can run ANY function of the
program: every menu item, toolbar button, tool action, panel button, dialog action and setting is a
**command**, and the MCP tools expose the command catalog, state queries and screenshots.

```
 AI client  <--stdio JSON-RPC-->  darkburn-mcp.exe  <--loopback TCP, token-->  DarkBurn app
                                  (tools, schemas)                             Gate -> Backend (UI thread)
```

Built on SolidCraft's small `solidcraft-mcp-server` crate (by path, unchanged) for the schemas,
argument checks and result content; the JSON-RPC loop is a copy with one change (see
`ATTRIBUTION.md`).

## Register the server

Claude Code:

```
claude mcp add darkburn -- C:\path\to\darkburn-mcp.exe
```

Claude Desktop (`claude_desktop_config.json`):

```json
{ "mcpServers": { "darkburn": { "command": "C:\\path\\to\\darkburn-mcp.exe", "args": [] } } }
```

Start DarkBurn first (or later: the server reconnects, and says "DarkBurn is not running" until it
is). It finds the app through `%APPDATA%\DarkBurn\control.json` (port 27879 by default plus a
random per-session token; the file is only readable by you). Options: `--control-file PATH`,
`--port N --token-file PATH`, `--headless` (an in-memory backend without a window, for CI; hardware
is always denied there), `--version`.

## Tools

| Tool | What it does |
|---|---|
| `list_commands`, `describe_command` | The catalog (filter, prefix, panel, compact, limit) and one command's schemas |
| `execute`, `batch` | Run any command / several in order, checked first, optionally all-or-nothing |
| `inspect_design`, `inspect_ui` | Shapes (stable ids), layers (index or name), selection, cut settings; panels, dialogs |
| `get_setting`, `set_setting`, `list_settings` | Settings |
| `new_design`, `open`, `save`, `import`, `export`, `undo`, `redo` | Shortcuts for `file.*` / `edit.*` commands |
| `screenshot` | PNG of the window or a region |
| `measure` | Bounding boxes, lengths, areas, estimated job time |
| `preview_job` | Toolpath summary or G-code text; never sends anything |
| `machine_status`, `list_devices` | Read only |
| `pending`, `running` | Long operations and confirmations |
| `events` | Command results, dialogs opened, errors, machine events and the audit log |
| `skill_reference`, `crash_reports` | The agent guide (`skill/darkburn-agent-guide.md`), crash reports |

Resources: `darkburn://checklist` (the parity checklist), `darkburn://agent-guide`,
`darkburn://commands` (compact catalog). All tool arguments are checked against their JSON Schema
(unknown arguments are errors); a command's `params` are checked against the command's own schema.

## Safety model

The **Gate** wraps the app's backend on the app side of the channel (a hostile client cannot skip
it). The human owns the policy (`McpPolicy`, Preferences > AI agents):

| Setting | Default | Effect |
|---|---|---|
| `allow_hardware` | off | Commands with `hardware: true` are denied until on |
| `allow_gweike` | off | Gweike commands also need this (on top of `allow_hardware`) |
| `require_confirmation_for_job_start` | on | Starting a job or firing needs a one-time token issued after the human clicks Confirm |
| `allow_file_write` | on | Commands with `writes_files: true` (save, export, machine files) |

* Emergency stop, pause and stop (`HardwareClass::Stop`) are always allowed.
* A denied command never reaches the backend; denial text tells the agent to ask the user.
* The confirmation token is bound to the command and its exact arguments, expires (5 min) and works
  once. Flow: `execute` returns `confirmation_required` + pending id; the gate calls
  `Backend::request_confirmation` so the app shows a dialog; the human clicks; the app calls
  `Gate::resolve_confirmation(id, true)`; the agent's `pending {id}` returns the token; the agent
  repeats the call with `confirmation_token`.
* A batch is checked step by step before any step runs.
* Every call is in the audit log (timestamp, tool, command, args hash, outcome), read via `events`.
* The control server binds 127.0.0.1 only, drops non-loopback peers, checks the token in constant
  time and accepts only a 4 KiB frame before login.

## Integrating it into the app

1. Implement `darkburn_mcp::Backend` on a type that lives on the UI thread. Required:
   `list_commands`, `describe_command`, `execute`, `inspect`. Everything else has a default
   (`screenshot`, `measure`, `preview_job`, `events`, `request_confirmation`, `checkpoint`/`restore`
   for rollback, ...). Fill `CommandInfo` for each command, including `hardware`,
   `hardware_class` (`Connect`, `Motion`, `Config`, `FireOrJob`, `Stop`), `gweike`, `writes_files`,
   `params_schema` (JSON Schema, root `"type": "object"`) and `checklist_ids`.
2. Register the commands the tools rely on: `darkburn_mcp::REQUIRED_COMMANDS`
   (`file.new/open/save/save_as/import/export`, `edit.undo/redo`, `settings.set`).
3. Wrap it: `let mut gate = Gate::new(backend, policy_handle.clone());` and keep `policy_handle`
   (a `PolicyHandle`) in your settings so the checkboxes call `policy_handle.update(..)`.
4. Start the server once: `let server = ControlServer::bind(ControlConfig { discovery_file:
   Some(control_file_path()), waker: Some(Arc::new(move || ctx.request_repaint())), ..Default::default() })?;`
   Keep it alive for the app's lifetime (dropping it closes the channel and removes `control.json`).
5. Once per frame on the UI thread: `server.pump(&mut gate, 32);`. Backend calls happen only there.
6. When the confirmation dialog (shown from `Backend::request_confirmation`) is answered, call
   `gate.resolve_confirmation(&pending_id, clicked_confirm)`. Never call it without a real click.
7. Add a test: `assert_full_coverage(&mut backend, &gui_action_ids)` with every action id the menus,
   ribbon, tool bar, panels and dialogs render. It fails on a GUI action without a command, an
   invalid `params_schema`, or a `describe_command` that differs from `list_commands`.

Threading contract (also in `backend.rs`): the backend is not `Send`; connection threads only queue
requests; `pump` runs them on the UI thread; every backend method returns promptly; a long operation
returns `CommandError::still_running(id, what)` and is reported by `pending(id)`/`running()`, which
only report and never wait.

```rust
use darkburn_mcp::*;
use serde_json::{json, Value};

struct App { /* document, settings, machine ... */ }

impl Backend for App {
    fn list_commands(&mut self, q: &ListQuery) -> Result<CommandPage, CommandError> {
        Ok(q.apply(self.catalog())) // Vec<CommandInfo>
    }
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        Ok(self.catalog().into_iter().find(|c| c.id == id))
    }
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        match id {
            "file.new" => { self.new_document(); Ok(json!({})) }
            "machine.jog" => self.jog(args),        // CommandInfo { hardware: true, hardware_class: Some(Motion), .. }
            _ => Err(CommandError::new(ErrorCode::UnknownCommand, id)),
        }
    }
    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> { /* design.*, settings.*, machine.*, ui.*, undo, job */ todo!() }
}

// In the UI setup:
let policy = PolicyHandle::default();                    // hardware off, confirmation on
let mut gate = Gate::new(App::new(), policy.clone());
let server = ControlServer::bind(ControlConfig { discovery_file: Some(control::control_file_path()), ..Default::default() })?;
// Each frame:
server.pump(&mut gate, 32);
```

`MockBackend` is a complete small example (shapes, layers, undo, settings, a simulated machine and
a long operation) and is what `--headless` serves until the app's own headless backend exists.

## Tests

`cargo test -p darkburn-mcp` covers protocol conformance, argument validation, batch semantics,
each safety rule (including "a denied command never reaches the backend"), the audit log, the
control channel (auth, loopback rules, oversized and garbage frames, timeouts, reconnect,
concurrency, large payloads) and the coverage helpers. Tests bind `127.0.0.1` on ephemeral ports
only and never open a device.
