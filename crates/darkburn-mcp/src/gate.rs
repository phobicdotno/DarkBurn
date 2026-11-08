//! The safety gate: middleware around a [`Backend`] that every MCP call must pass.
//!
//! Rules (each has a test in `tests/safety.rs`):
//!
//! 1. A command with `hardware == true` is denied unless the human turned on
//!    `allow_hardware`. A denied command never reaches the wrapped backend's `execute`.
//! 2. Stop-class commands (emergency stop, pause, stop) are always allowed.
//! 3. A hardware command flagged `gweike` also needs `allow_gweike` (stop excepted).
//! 4. A `FireOrJob` command (starts a job, fires the laser) needs, when
//!    `require_confirmation_for_job_start` is on, a one-time token issued after the human clicked
//!    Confirm in the app's dialog. The token is bound to the command and its exact arguments,
//!    expires, and works once.
//! 5. A command that writes files is denied unless `allow_file_write`.
//! 6. Parameters are validated against the command's JSON Schema before the backend runs.
//! 7. A batch is checked step by step before ANY step runs; one denied step rejects the batch.
//! 8. Every call is written to the audit log (timestamp, tool, command, args hash, outcome),
//!    readable through `events`.
//!
//! The gate lives on the app side of the control channel, so a hostile client cannot skip it.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::backend::Backend;
use crate::policy::{McpPolicy, PolicyHandle};
use crate::types::*;
use crate::util::{random_hex, validate_instance};

const MAX_LOG: usize = 10_000;

enum ConfirmState {
    Waiting,
    Approved { token: String, issued: Instant },
    Denied,
}

struct Confirmation {
    command: String,
    args_hash: String,
    summary: String,
    created: Instant,
    state: ConfirmState,
}

struct Token {
    command: String,
    args_hash: String,
    issued: Instant,
    confirmation: String,
}

/// What the policy says about one command.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    NeedsConfirmation,
    Deny(String),
}

pub struct Gate<B: Backend> {
    inner: B,
    policy: PolicyHandle,
    log: VecDeque<Event>,
    seq: u64,
    inner_cursor: u64,
    confirmations: HashMap<String, Confirmation>,
    tokens: HashMap<String, Token>,
    token_ttl: Duration,
    waiting_ttl: Duration,
}

impl<B: Backend> Gate<B> {
    pub fn new(inner: B, policy: PolicyHandle) -> Self {
        Self {
            inner,
            policy,
            log: VecDeque::new(),
            seq: 0,
            inner_cursor: 0,
            confirmations: HashMap::new(),
            tokens: HashMap::new(),
            token_ttl: Duration::from_secs(300),
            waiting_ttl: Duration::from_secs(600),
        }
    }

    pub fn policy(&self) -> &PolicyHandle {
        &self.policy
    }
    /// The wrapped backend (for the app's own use; calls through it bypass the gate on purpose).
    pub fn inner(&mut self) -> &mut B {
        &mut self.inner
    }
    pub fn into_inner(self) -> B {
        self.inner
    }
    /// How long a confirmation token stays valid after the human confirmed (default 5 minutes).
    pub fn set_token_ttl(&mut self, ttl: Duration) {
        self.token_ttl = ttl;
    }

    fn push(&mut self, mut e: Event) {
        self.seq += 1;
        e.seq = self.seq;
        e.ts_ms = now_ms();
        if self.log.len() >= MAX_LOG {
            self.log.pop_front();
        }
        self.log.push_back(e);
    }

    fn audit(&mut self, tool: &str, command: &str, args: &Value, outcome: &str, message: &str) {
        self.push(Event {
            seq: 0,
            ts_ms: 0,
            kind: "audit".into(),
            tool: tool.into(),
            command: command.into(),
            args_hash: args_hash(args),
            outcome: outcome.into(),
            message: message.chars().take(300).collect(),
            data: Value::Null,
        });
    }

    fn audit_result<T>(&mut self, tool: &str, command: &str, args: &Value, r: &Result<T, CommandError>) {
        match r {
            Ok(_) => self.audit(tool, command, args, "ok", ""),
            Err(e) => self.audit(tool, command, args, &format!("{:?}", e.code).to_lowercase(), &e.message),
        }
    }

    fn verdict(&self, info: &CommandInfo) -> Verdict {
        let p: McpPolicy = self.policy.get();
        let class = info.effective_class();
        if class == Some(HardwareClass::Stop) {
            return Verdict::Allow;
        }
        if class.is_some() {
            if !p.allow_hardware {
                return Verdict::Deny(format!(
                    "`{}` controls the machine and AI agents may not do that: ask the user to turn on \"Allow AI agents to control machines\" in DarkBurn's settings (Preferences > AI agents). Until then only emergency stop, pause and stop are available to you.",
                    info.id
                ));
            }
            if info.gweike && !p.allow_gweike {
                return Verdict::Deny(format!(
                    "`{}` talks to the Gweike machine and the Gweike option is off: ask the user to turn on \"Allow AI agents to use Gweike machines\" in DarkBurn's settings.",
                    info.id
                ));
            }
        }
        if info.writes_files && !p.allow_file_write {
            return Verdict::Deny(format!(
                "`{}` writes files and AI agents may not do that: ask the user to turn on \"Allow AI agents to write files\" in DarkBurn's settings.",
                info.id
            ));
        }
        if class == Some(HardwareClass::FireOrJob) && p.require_confirmation_for_job_start {
            return Verdict::NeedsConfirmation;
        }
        Verdict::Allow
    }

    fn lookup(&mut self, id: &str) -> Result<CommandInfo, CommandError> {
        match self.inner.describe_command(id)? {
            Some(i) => Ok(i),
            None => Err(CommandError::new(
                ErrorCode::UnknownCommand,
                format!("unknown command `{id}`; use list_commands with a filter to find the right id"),
            )),
        }
    }

    fn check_params(info: &CommandInfo, params: &Value) -> Result<Value, CommandError> {
        let params = if params.is_null() { Value::Object(Default::default()) } else { params.clone() };
        if !params.is_object() {
            return Err(CommandError::invalid(format!("`{}`: params must be an object", info.id)));
        }
        validate_instance(&info.schema_or_empty(), &params)
            .map_err(|e| CommandError::invalid(format!("`{}`: invalid params: {e}. describe_command shows the schema.", info.id)))?;
        Ok(params)
    }

    fn purge(&mut self) {
        let (ttl, wttl) = (self.token_ttl, self.waiting_ttl);
        self.tokens.retain(|_, t| t.issued.elapsed() <= ttl);
        self.confirmations.retain(|_, c| match &c.state {
            ConfirmState::Waiting => c.created.elapsed() <= wttl,
            ConfirmState::Approved { issued, .. } => issued.elapsed() <= ttl,
            ConfirmState::Denied => true,
        });
    }

    /// Open a confirmation for a command and ask the app to show its dialog.
    fn open_confirmation(&mut self, info: &CommandInfo, params: &Value) -> CommandError {
        let pid = format!("c{}", random_hex(4));
        let mut shown = params.to_string();
        if shown.len() > 200 {
            shown = format!("{}...", shown.chars().take(200).collect::<String>());
        }
        let summary = format!("{} ({}) with {shown}", info.title, info.id);
        if let Err(e) = self.inner.request_confirmation(&pid, &info.id, &summary) {
            return CommandError::denied(format!("`{}` needs the user's confirmation but the app could not show the dialog: {}", info.id, e.message));
        }
        self.confirmations.insert(
            pid.clone(),
            Confirmation { command: info.id.clone(), args_hash: args_hash(params), summary, created: Instant::now(), state: ConfirmState::Waiting },
        );
        self.push(Event {
            seq: 0,
            ts_ms: 0,
            kind: "confirmation".into(),
            tool: String::new(),
            command: info.id.clone(),
            args_hash: args_hash(params),
            outcome: "requested".into(),
            message: pid.clone(),
            data: Value::Null,
        });
        CommandError {
            code: ErrorCode::ConfirmationRequired,
            message: format!(
                "`{}` needs the user's confirmation: a dialog is open in DarkBurn. Tell the user to click Confirm, then call `pending` with id `{pid}` (it returns a confirmation_token once the user confirmed), then repeat this call with `confirmation_token` set. The token works once, only for these exact arguments.",
                info.id
            ),
            pending_id: Some(pid),
        }
    }

    /// Check a token without consuming it.
    fn peek_token(&self, token: &str, info: &CommandInfo, params: &Value) -> Result<(), CommandError> {
        let t = self.tokens.get(token).ok_or_else(|| {
            CommandError::new(
                ErrorCode::ConfirmationInvalid,
                "unknown, used or expired confirmation_token: request a new confirmation by repeating the call without a token",
            )
        })?;
        if t.issued.elapsed() > self.token_ttl {
            return Err(CommandError::new(
                ErrorCode::ConfirmationInvalid,
                "the confirmation_token expired: repeat the call without a token to ask the user again",
            ));
        }
        if t.command != info.id || t.args_hash != args_hash(params) {
            return Err(CommandError::new(
                ErrorCode::ConfirmationInvalid,
                "the confirmation_token was issued for a different command or different arguments: the user must confirm exactly what runs",
            ));
        }
        Ok(())
    }

    fn consume_token(&mut self, token: &str) {
        if let Some(t) = self.tokens.remove(token) {
            self.confirmations.remove(&t.confirmation);
        }
    }

    /// The human clicked Confirm (`approved`) or Cancel in the dialog tagged `pending_id`. Called
    /// by the app's dialog code, on the UI thread. Never call it for an agent's request without a
    /// real click.
    pub fn resolve_confirmation(&mut self, pending_id: &str, approved: bool) -> Result<(), String> {
        self.purge();
        let c = self.confirmations.get_mut(pending_id).ok_or_else(|| format!("no open confirmation `{pending_id}` (expired or already resolved)"))?;
        if !matches!(c.state, ConfirmState::Waiting) {
            return Err(format!("confirmation `{pending_id}` is already resolved"));
        }
        let (command, hash) = (c.command.clone(), c.args_hash.clone());
        if approved {
            let token = random_hex(16);
            c.state = ConfirmState::Approved { token: token.clone(), issued: Instant::now() };
            self.tokens
                .insert(token, Token { command: command.clone(), args_hash: hash.clone(), issued: Instant::now(), confirmation: pending_id.into() });
        } else {
            c.state = ConfirmState::Denied;
        }
        self.push(Event {
            seq: 0,
            ts_ms: 0,
            kind: "confirmation".into(),
            tool: String::new(),
            command,
            args_hash: hash,
            outcome: if approved { "approved" } else { "denied" }.into(),
            message: pending_id.into(),
            data: Value::Null,
        });
        Ok(())
    }

    /// Open confirmation dialogs (id, command, summary), for the app's own UI.
    pub fn open_confirmations(&self) -> Vec<(String, String, String)> {
        self.confirmations
            .iter()
            .filter(|(_, c)| matches!(c.state, ConfirmState::Waiting))
            .map(|(k, c)| (k.clone(), c.command.clone(), c.summary.clone()))
            .collect()
    }

    fn run_execute(&mut self, id: &str, args: Value, ctx: &CallContext) -> Result<Value, CommandError> {
        self.purge();
        let info = self.lookup(id)?;
        let params = Self::check_params(&info, &args)?;
        if !info.enabled {
            return Err(CommandError::new(
                ErrorCode::Disabled,
                format!("`{id}` is disabled right now (nothing selected, wrong tool, or no document); inspect_design / inspect_ui show why"),
            ));
        }
        match self.verdict(&info) {
            Verdict::Deny(m) => return Err(CommandError::denied(m)),
            Verdict::Allow => {}
            Verdict::NeedsConfirmation => match ctx.confirmation_token.as_deref() {
                None => return Err(self.open_confirmation(&info, &params)),
                Some(t) => {
                    self.peek_token(t, &info, &params)?;
                    self.consume_token(t);
                }
            },
        }
        self.inner.execute(id, params)
    }

    fn run_batch(&mut self, steps: &[BatchStep], all_or_nothing: bool) -> Result<BatchResult, CommandError> {
        self.purge();
        let mut clean = Vec::with_capacity(steps.len());
        let mut used: Vec<String> = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            let at =
                |e: CommandError| CommandError { message: format!("batch rejected, nothing ran. Step {i} (`{}`): {}", s.command, e.message), ..e };
            let info = self.lookup(&s.command).map_err(at)?;
            let params = Self::check_params(&info, &s.params).map_err(at)?;
            match self.verdict(&info) {
                Verdict::Deny(m) => return Err(at(CommandError::denied(m))),
                Verdict::Allow => {}
                Verdict::NeedsConfirmation => match s.confirmation_token.as_deref() {
                    None => return Err(at(self.open_confirmation(&info, &params))),
                    Some(t) => {
                        if used.iter().any(|u| u == t) {
                            return Err(at(CommandError::new(
                                ErrorCode::ConfirmationInvalid,
                                "the same confirmation_token is used by two steps; each needs its own",
                            )));
                        }
                        self.peek_token(t, &info, &params).map_err(at)?;
                        used.push(t.to_string());
                    }
                },
            }
            clean.push(BatchStep { command: s.command.clone(), params, confirmation_token: None });
        }
        for t in &used {
            self.consume_token(t);
        }
        self.inner.batch(&clean, all_or_nothing)
    }
}

impl<B: Backend> Backend for Gate<B> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError> {
        self.inner.list_commands(query)
    }

    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError> {
        self.inner.describe_command(id)
    }

    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError> {
        self.execute_ctx(id, args, &CallContext { tool: "execute".into(), confirmation_token: None })
    }

    fn execute_ctx(&mut self, id: &str, args: Value, ctx: &CallContext) -> Result<Value, CommandError> {
        let tool = if ctx.tool.is_empty() { "execute" } else { ctx.tool.as_str() }.to_string();
        let r = self.run_execute(id, args.clone(), ctx);
        self.audit_result(&tool, id, &args, &r);
        r
    }

    fn batch(&mut self, steps: &[BatchStep], all_or_nothing: bool) -> Result<BatchResult, CommandError> {
        let r = self.run_batch(steps, all_or_nothing);
        match &r {
            Ok(b) => {
                for s in &b.steps {
                    let args = steps.get(s.index).map_or(Value::Null, |x| x.params.clone());
                    match &s.error {
                        None => self.audit("batch", &s.command, &args, "ok", ""),
                        Some(e) => self.audit("batch", &s.command, &args, &format!("{:?}", e.code).to_lowercase(), &e.message),
                    }
                }
                if b.rolled_back {
                    self.audit("batch", "", &Value::Null, "rolled_back", "all-or-nothing batch undone");
                }
            }
            Err(e) => {
                let msg = e.message.clone();
                let code = format!("{:?}", e.code).to_lowercase();
                self.audit("batch", "", &json!({"steps": steps.len()}), &code, &msg);
            }
        }
        r
    }

    fn checkpoint(&mut self) -> Result<Option<String>, CommandError> {
        self.inner.checkpoint()
    }
    fn restore(&mut self, checkpoint: &str) -> Result<(), CommandError> {
        self.inner.restore(checkpoint)
    }
    fn discard_checkpoint(&mut self, checkpoint: &str) {
        self.inner.discard_checkpoint(checkpoint)
    }

    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError> {
        let r = if path == "mcp" || path == "mcp.policy" {
            Ok(json!({"policy": self.policy.get(), "open_confirmations": self.open_confirmations().len()}))
        } else {
            self.inner.inspect(path, depth)
        };
        self.audit_result("inspect", path, &Value::Null, &r);
        r
    }

    fn screenshot(&mut self, region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        let r = self.inner.screenshot(region);
        self.audit_result("screenshot", "", &json!(region), &r);
        r
    }

    fn measure(&mut self, request: Value) -> Result<Value, CommandError> {
        let r = self.inner.measure(request.clone());
        self.audit_result("measure", "", &request, &r);
        r
    }

    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        let r = self.inner.preview_job(request.clone());
        self.audit_result("preview_job", "", &request, &r);
        r
    }

    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        self.purge();
        if let Some(c) = self.confirmations.get(id) {
            return Ok(match &c.state {
                ConfirmState::Waiting => PendingState::Running {
                    what: format!("waiting for the user to confirm: {}", c.summary),
                    running_s: c.created.elapsed().as_secs(),
                },
                ConfirmState::Approved { token, issued } => PendingState::Done {
                    what: c.summary.clone(),
                    result: Ok(
                        json!({"confirmed": true, "confirmation_token": token, "command": c.command, "expires_in_s": self.token_ttl.saturating_sub(issued.elapsed()).as_secs()}),
                    ),
                },
                ConfirmState::Denied => {
                    let what = c.summary.clone();
                    self.confirmations.remove(id);
                    PendingState::Done {
                        what,
                        result: Err(CommandError::new(
                            ErrorCode::ConfirmationDenied,
                            "the user cancelled the confirmation dialog: do not retry unless the user asks",
                        )),
                    }
                }
            });
        }
        self.inner.pending(id)
    }

    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        self.purge();
        let mut v = self.inner.running()?;
        for (k, c) in &self.confirmations {
            if matches!(c.state, ConfirmState::Waiting) {
                v.push(RunningOp {
                    id: k.clone(),
                    what: format!("waiting for the user to confirm: {}", c.summary),
                    running_s: c.created.elapsed().as_secs(),
                });
            }
        }
        Ok(v)
    }

    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        // Pull the app's own events into the one log, so a reader sees one ordered stream.
        if let Ok(page) = self.inner.events(self.inner_cursor) {
            self.inner_cursor = page.next.max(self.inner_cursor);
            for mut e in page.events {
                if e.kind == "audit" {
                    e.kind = "app".into();
                }
                self.push(e);
            }
        }
        let events: Vec<Event> = self.log.iter().filter(|e| e.seq > since).cloned().collect();
        Ok(EventPage { events, next: self.seq })
    }

    fn crash_reports(&mut self, limit: usize) -> Result<Value, CommandError> {
        let r = self.inner.crash_reports(limit);
        self.audit_result("crash_reports", "", &Value::Null, &r);
        r
    }

    fn request_confirmation(&mut self, _pending_id: &str, _command: &str, _summary: &str) -> Result<(), CommandError> {
        Err(CommandError::denied("confirmations are opened by the gate itself"))
    }
}
