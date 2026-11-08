# Attribution for darkburn-mcp

## SolidCraft (MIT OR Apache-2.0)

Source: https://github.com/tangency-labs/solidcraft (fork https://github.com/phobicdotno/solidcraft),
Copyright (c) 2026 SolidCraft contributors. Licence and notice: `licenses/SolidCraft-LICENSE-MIT`,
`licenses/SolidCraft-NOTICE` in the DarkBurn root.

* **Used as a crate, by path, unchanged:** `solidcraft-mcp-server` (`../solidcraft/crates/mcp-server`):
  `tool`, `object`, `string`, `Effect`, `check_arguments`, `text_result`, `image_result`, `base64`.
* **Copied with one change:** `src/server.rs` is `crates/mcp-server/src/server.rs` (JSON-RPC 2.0
  framing, MCP lifecycle, tools and resources methods). Change: `Tools::read_resource` returns a
  `Resource { mime, text }` instead of a JSON value, so Markdown resources are served as Markdown.
  The SolidCraft crate could not do that without editing it.
* **Design followed, no code copied:** the tool set and tone (`list_commands`, `describe_command`,
  `execute`, `batch`, `inspect_design`, `pending`, `running`, `skill_reference`, ...) follow
  `crates/mcp` and `docs/mcp.md`; the "still running:" convention follows `backend.rs`.

## Other dependencies (all MIT OR Apache-2.0)

`serde`, `serde_json`, `getrandom`, `jsonschema` (MIT).
The official Rust SDK `rmcp` was considered; SolidCraft's server is smaller, has no async runtime
and already fits, so it was not added.
