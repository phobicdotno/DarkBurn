//! Plain data types shared by the backend, the gate, the tools and the control channel.
//! Everything is JSON-serialisable, so the crate depends on no app internals.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What kind of hardware effect a command has. Set by the app. The gate decides on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareClass {
    /// Opens or closes a connection to a device, or scans one.
    Connect,
    /// Moves axes (jog, home, frame, go to origin, set origin on the machine).
    Motion,
    /// Writes machine settings or controller files (card registers, EEPROM, config upload).
    Config,
    /// Starts a job, runs a stored job, or fires the laser (test fire, pulse).
    FireOrJob,
    /// Emergency stop, pause, resume-safe stop, stop. Always allowed, whatever the policy says.
    Stop,
}

/// A registered command, as `list_commands` and `describe_command` report it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandInfo {
    /// Stable dotted lower-case id, for example `file.exit` or `edit.align_left`.
    pub id: String,
    pub title: String,
    /// Group or panel the command belongs to (`File`, `Edit`, `Cuts / Layers`, `Laser`).
    #[serde(default)]
    pub group: String,
    /// Where the GUI shows it, for example `["File", "Save"]`; empty when it has no menu entry.
    #[serde(default)]
    pub menu_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// Enabled right now (a disabled command fails with `disabled`).
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Moves axes, fires the laser, sends jobs, writes machine settings or controller files, or
    /// connects to a device. The safety gate denies these until the human allows them.
    #[serde(default)]
    pub hardware: bool,
    /// What the hardware effect is. `None` on a hardware command means "unspecified": treated as
    /// the most restrictive non-stop class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_class: Option<HardwareClass>,
    /// Related to the Gweike M3 Ultra / MCC100 card. Denied unless `allow_gweike` is on.
    #[serde(default)]
    pub gweike: bool,
    /// Writes files outside the app's own state (save, export, machine files).
    #[serde(default)]
    pub writes_files: bool,
    /// JSON Schema of the `params` object. `Null` means "no parameters".
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params_schema: Value,
    /// JSON Schema of the result.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result_schema: Value,
    /// `docs/PARITY-CHECKLIST.md` ids this command implements, for example `FILE-20`.
    #[serde(default)]
    pub checklist_ids: Vec<String>,
    /// One-paragraph documentation for `describe_command`; absent in compact listings.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

fn yes() -> bool {
    true
}

impl CommandInfo {
    /// A minimal safe command (no hardware, enabled, no parameters).
    pub fn new(id: &str, title: &str, group: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            group: group.into(),
            menu_path: Vec::new(),
            shortcut: None,
            enabled: true,
            hardware: false,
            hardware_class: None,
            gweike: false,
            writes_files: false,
            params_schema: Value::Null,
            result_schema: Value::Null,
            checklist_ids: Vec::new(),
            doc: String::new(),
        }
    }

    /// The effective class: `Some` for hardware commands (unspecified means `Motion`).
    pub fn effective_class(&self) -> Option<HardwareClass> {
        if self.hardware || self.hardware_class.is_some() { Some(self.hardware_class.unwrap_or(HardwareClass::Motion)) } else { None }
    }

    /// The compact form: id, title, enabled, hardware flag only.
    pub fn compact(&self) -> Value {
        json!({"id": self.id, "title": self.title, "group": self.group, "enabled": self.enabled, "hardware": self.hardware})
    }

    /// The params schema as a usable schema (`Null` becomes the empty closed object).
    pub fn schema_or_empty(&self) -> Value {
        if self.params_schema.is_null() {
            json!({"type": "object", "properties": {}, "additionalProperties": false})
        } else {
            self.params_schema.clone()
        }
    }
}

/// The filters of `list_commands`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ListQuery {
    /// Case-insensitive substring over id, title, group and menu path.
    #[serde(default)]
    pub filter: Option<String>,
    /// Start of the id.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Whole group / panel name, case-insensitive.
    #[serde(default)]
    pub panel: Option<String>,
    /// Omit schemas and docs.
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// One page of the catalog.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandPage {
    pub commands: Vec<CommandInfo>,
    /// Matches before paging.
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

impl ListQuery {
    /// Filter, sort by id, compact and page `all`. Backends may call this instead of doing it
    /// themselves.
    pub fn apply(&self, all: Vec<CommandInfo>) -> CommandPage {
        let f = self.filter.as_ref().map(|s| s.to_lowercase());
        let mut v: Vec<CommandInfo> = all
            .into_iter()
            .filter(|c| {
                self.prefix.as_ref().is_none_or(|p| c.id.starts_with(p.as_str()))
                    && self.panel.as_ref().is_none_or(|p| c.group.eq_ignore_ascii_case(p))
                    && f.as_ref().is_none_or(|f| {
                        c.id.to_lowercase().contains(f)
                            || c.title.to_lowercase().contains(f)
                            || c.group.to_lowercase().contains(f)
                            || c.menu_path.join("/").to_lowercase().contains(f)
                    })
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        let total = v.len();
        let offset = self.offset.unwrap_or(0).min(total);
        let end = self.limit.map_or(total, |l| offset.saturating_add(l).min(total));
        let mut commands: Vec<CommandInfo> = v.drain(offset..end).collect();
        if self.compact {
            for c in &mut commands {
                c.params_schema = Value::Null;
                c.result_schema = Value::Null;
                c.doc.clear();
            }
        }
        CommandPage { commands, total, next_offset: (end < total).then_some(end) }
    }
}

/// Why a command did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownCommand,
    InvalidParams,
    Disabled,
    Failed,
    NotSupported,
    /// Refused by the safety gate (policy).
    Denied,
    /// A human must confirm first; `pending_id` names the confirmation.
    ConfirmationRequired,
    ConfirmationInvalid,
    ConfirmationDenied,
    /// Not finished; `pending_id` names it. Not cancelled.
    StillRunning,
    /// The control channel failed (app not running, bad token, connection lost).
    Transport,
    Timeout,
}

/// A failed (or not yet finished) command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandError {
    pub code: ErrorCode,
    /// Actionable text: what happened and what to do about it.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_id: Option<String>,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), pending_id: None }
    }
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Failed, message)
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, message)
    }
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Denied, message)
    }
    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Transport, message)
    }
    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorCode::NotSupported, format!("{what} is not supported by this backend"))
    }
    /// A long operation that was started and is still running.
    pub fn still_running(pending_id: impl Into<String>, what: &str) -> Self {
        let id = pending_id.into();
        Self {
            code: ErrorCode::StillRunning,
            message: format!("still running: {what}; it was not cancelled, call `pending` with id `{id}`"),
            pending_id: Some(id),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for CommandError {}

/// One step of a batch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BatchStep {
    pub command: String,
    #[serde(default)]
    pub params: Value,
    /// One-time token for a command that needs the human's confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_token: Option<String>,
}

/// What happened to one step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepResult {
    pub index: usize,
    pub command: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CommandError>,
}

/// The outcome of a batch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchResult {
    pub ok: bool,
    /// Results of the steps that ran (a failed step is the last).
    pub steps: Vec<StepResult>,
    /// Index of the failing step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_step: Option<usize>,
    /// The batch was all-or-nothing and the earlier steps were undone.
    #[serde(default)]
    pub rolled_back: bool,
    /// Steps not run because an earlier one failed.
    #[serde(default)]
    pub skipped: usize,
}

/// Who/what is calling: set by the tool layer so the audit log names the MCP tool.
#[derive(Clone, Debug, Default)]
pub struct CallContext {
    pub tool: String,
    pub confirmation_token: Option<String>,
}

/// State of a long operation or a confirmation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PendingState {
    /// No such id (never existed, or already collected).
    Unknown,
    Running {
        what: String,
        running_s: u64,
    },
    Done {
        what: String,
        result: Result<Value, CommandError>,
    },
}

/// One running long operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunningOp {
    pub id: String,
    pub what: String,
    pub running_s: u64,
}

/// One entry of the event log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub ts_ms: u64,
    /// `audit`, `command`, `dialog`, `error`, `machine`, `confirmation`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub args_hash: String,
    /// `ok`, `denied`, `error`, `confirmation_required`, ...
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub outcome: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// A page of events.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventPage {
    pub events: Vec<Event>,
    /// Pass as `since` to get only newer events.
    pub next: u64,
}

/// A screen region in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// FNV-1a 64 of the compact JSON text, as 16 hex digits (the audit log's args hash). Not
/// cryptographic: it only lets a reader tell calls apart without storing their contents.
pub fn args_hash(args: &Value) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in args.to_string().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}
