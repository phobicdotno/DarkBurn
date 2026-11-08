//! The [`Backend`] trait: everything the MCP layer needs from DarkBurn, all JSON-based.
//!
//! # Threading contract
//!
//! A `Backend` is single-threaded and not `Send`: it lives on the app's UI thread, next to the
//! document. The control server ([`crate::control::ControlServer`]) accepts connections on its own
//! threads but never touches the backend; it queues each request and the UI thread runs
//! [`crate::control::ControlServer::pump`] once per frame, which calls the backend (through the
//! [`crate::Gate`]) and sends the reply back. Consequences:
//!
//! * every method must return promptly (no blocking waits, no sleeping): a long operation
//!   (a boolean on a huge design, tracing an image, generating a job) is started and the method
//!   returns [`CommandError::still_running`] with a pending id; [`Backend::pending`] is polled;
//! * `pending` and `running` only report, they never wait (the tool layer does the waiting);
//! * a backend method may open a dialog, but must not block on it: a dialog the human has to
//!   answer is reported through `events` and `inspect("ui")`.

use serde_json::Value;

use crate::types::*;

pub trait Backend {
    /// Short description for `initialize` (for example `DarkBurn 0.1 window`, `headless`).
    fn describe(&self) -> String {
        "DarkBurn".into()
    }

    /// The catalog, filtered and paged by `query` (see [`ListQuery::apply`]).
    fn list_commands(&mut self, query: &ListQuery) -> Result<CommandPage, CommandError>;

    /// One command, with schemas and docs. `None` when the id is unknown.
    fn describe_command(&mut self, id: &str) -> Result<Option<CommandInfo>, CommandError>;

    /// Run a command. Any menu item, toolbar button, tool action, panel button, dialog action or
    /// setting change is a command. A long operation returns `Err(CommandError::still_running)`.
    fn execute(&mut self, id: &str, args: Value) -> Result<Value, CommandError>;

    /// Like [`Backend::execute`] with the caller's context (tool name, confirmation token). Only
    /// the [`crate::Gate`] uses the context; others keep the default.
    fn execute_ctx(&mut self, id: &str, args: Value, ctx: &CallContext) -> Result<Value, CommandError> {
        let _ = ctx;
        self.execute(id, args)
    }

    /// Run `steps` in order. With `all_or_nothing`, a failing step undoes the earlier ones when the
    /// backend can ([`Backend::checkpoint`]); the reply says whether it did. The default checks
    /// that every id exists first, then runs the steps through [`Backend::execute`].
    fn batch(&mut self, steps: &[BatchStep], all_or_nothing: bool) -> Result<BatchResult, CommandError> {
        default_batch(self, steps, all_or_nothing)
    }

    /// Mark a state to go back to (the whole document + selection + settings). `Ok(None)`: this
    /// backend cannot roll back.
    fn checkpoint(&mut self) -> Result<Option<String>, CommandError> {
        Ok(None)
    }
    /// Go back to a checkpoint from [`Backend::checkpoint`] and drop it.
    fn restore(&mut self, checkpoint: &str) -> Result<(), CommandError> {
        let _ = checkpoint;
        Err(CommandError::unsupported("restore"))
    }
    /// Drop a checkpoint that is not needed.
    fn discard_checkpoint(&mut self, checkpoint: &str) {
        let _ = checkpoint;
    }

    /// State query. `path` is a dotted selector whose first segment is the root:
    /// `design` (shapes, layers, selection, transforms, cut settings), `settings`, `machine`
    /// (`machine.status`, `machine.devices`), `ui` (panels, dialogs, focus), `undo`, `job`.
    /// Shapes are addressed by stable id (`design.shapes.s12`), layers by index or name
    /// (`design.layers.0`, `design.layers.Cut`). `depth` limits nesting of the reply.
    fn inspect(&mut self, path: &str, depth: u32) -> Result<Value, CommandError>;

    /// PNG bytes of the window, or of `region`.
    fn screenshot(&mut self, region: Option<Region>) -> Result<Vec<u8>, CommandError> {
        let _ = region;
        Err(CommandError::unsupported("screenshot"))
    }

    /// Bounding boxes, lengths, areas, estimated job time. `request` is the tool's argument
    /// object (`target`, `what`).
    fn measure(&mut self, request: Value) -> Result<Value, CommandError> {
        let _ = request;
        Err(CommandError::unsupported("measure"))
    }

    /// Toolpath summary or G-code text of the current job, never sent anywhere.
    fn preview_job(&mut self, request: Value) -> Result<Value, CommandError> {
        let _ = request;
        Err(CommandError::unsupported("preview_job"))
    }

    /// State of a long operation or confirmation. Never waits.
    fn pending(&mut self, id: &str) -> Result<PendingState, CommandError> {
        let _ = id;
        Ok(PendingState::Unknown)
    }

    /// Long operations running now.
    fn running(&mut self) -> Result<Vec<RunningOp>, CommandError> {
        Ok(Vec::new())
    }

    /// Log of command results, dialogs opened, errors and machine events with `seq > since`.
    fn events(&mut self, since: u64) -> Result<EventPage, CommandError> {
        let _ = since;
        Ok(EventPage::default())
    }

    /// Crash reports found on disk (`limit` newest).
    fn crash_reports(&mut self, limit: usize) -> Result<Value, CommandError> {
        let _ = limit;
        Ok(Value::Array(Vec::new()))
    }

    /// Show the human a dialog "AI agent wants to: `summary`" with Confirm and Cancel, tagged
    /// `pending_id`. When the human clicks, the app calls [`crate::Gate::resolve_confirmation`].
    /// Called only by the gate. The default refuses, so nothing can start without a dialog.
    fn request_confirmation(&mut self, pending_id: &str, command: &str, summary: &str) -> Result<(), CommandError> {
        let _ = (pending_id, command, summary);
        Err(CommandError::unsupported("confirmation dialogs"))
    }
}

/// The default [`Backend::batch`].
pub fn default_batch<B: Backend + ?Sized>(b: &mut B, steps: &[BatchStep], all_or_nothing: bool) -> Result<BatchResult, CommandError> {
    // Unknown ids fail the whole batch before anything runs.
    for (i, s) in steps.iter().enumerate() {
        if b.describe_command(&s.command)?.is_none() {
            return Err(CommandError::new(
                ErrorCode::UnknownCommand,
                format!("batch step {i}: unknown command `{}`; use list_commands with a filter to find its id", s.command),
            ));
        }
    }
    let checkpoint = if all_or_nothing { b.checkpoint()? } else { None };
    let mut out = BatchResult { ok: true, ..Default::default() };
    for (i, s) in steps.iter().enumerate() {
        let params = if s.params.is_null() { Value::Object(Default::default()) } else { s.params.clone() };
        match b.execute(&s.command, params) {
            Ok(result) => out.steps.push(StepResult { index: i, command: s.command.clone(), ok: true, result, error: None }),
            Err(e) => {
                out.steps.push(StepResult { index: i, command: s.command.clone(), ok: false, result: Value::Null, error: Some(e) });
                out.ok = false;
                out.failed_step = Some(i);
                out.skipped = steps.len() - i - 1;
                break;
            }
        }
    }
    if let Some(cp) = checkpoint {
        if out.ok {
            b.discard_checkpoint(&cp);
        } else {
            out.rolled_back = b.restore(&cp).is_ok();
        }
    }
    Ok(out)
}

/// Commands the tool layer relies on. A real backend registers all of them
/// ([`crate::coverage::coverage_report`] checks).
pub const REQUIRED_COMMANDS: &[&str] =
    &["file.new", "file.open", "file.save", "file.save_as", "file.import", "file.export", "edit.undo", "edit.redo", "settings.set"];
