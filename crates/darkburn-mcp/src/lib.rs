//! MCP (Model Context Protocol) layer for DarkBurn: an AI agent can run ANY function of the
//! program. See the crate README for registration and the integration contract.
//!
//! * [`Backend`]: what the app implements (JSON-based, no app types).
//! * [`Gate`] + [`McpPolicy`]: the safety gate every call passes (hardware denied by default).
//! * [`tools`]: the MCP tools over a backend; [`server`] speaks the protocol.
//! * [`control`]: the loopback control channel (server half in the app, client half in the
//!   `darkburn-mcp` binary).
//! * [`MockBackend`]: in-memory backend for tests and `--headless`.
//! * [`coverage`]: guards that every GUI action has a command.

pub mod backend;
pub mod config;
pub mod control;
pub mod coverage;
pub mod gate;
pub mod mock;
pub mod policy;
pub mod server;
pub mod tools;
pub mod types;
pub mod util;

pub use backend::{Backend, REQUIRED_COMMANDS, default_batch};
pub use control::{ControlClient, ControlConfig, ControlServer, RemoteBackend};
pub use coverage::{CoverageReport, assert_full_coverage, coverage_report};
pub use gate::Gate;
pub use mock::MockBackend;
pub use policy::{McpPolicy, PolicyHandle};
pub use tools::{DarkburnTools, server as mcp_server, tool_definitions};
pub use types::*;
