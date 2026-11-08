//! Coverage guards: keep "any function is reachable through MCP" true.
//!
//! The app collects the ids of every action its GUI renders (menus, ribbon, tool bar, panels,
//! dialogs) and calls [`assert_full_coverage`] from a test. A GUI action without a command, a
//! command with an invalid parameter schema, or a `describe_command` that disagrees with
//! `list_commands` fails the test.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::backend::{Backend, REQUIRED_COMMANDS};
use crate::types::*;
use crate::util::check_schema_document;

#[derive(Clone, Debug, Default, Serialize)]
pub struct CoverageReport {
    pub registered: usize,
    pub gui_actions: usize,
    /// GUI actions with no command registered. Must be empty.
    pub missing: Vec<String>,
    /// Commands the MCP tools rely on (`REQUIRED_COMMANDS`) that are not registered.
    pub required_missing: Vec<String>,
    /// Commands that no GUI action maps to (informational: scripting-only commands are fine).
    pub not_in_gui: Vec<String>,
    /// Commands whose `params_schema` / `result_schema` is not a valid JSON Schema object.
    pub schema_errors: Vec<String>,
    /// Commands for which `describe_command` differs from the `list_commands` entry.
    pub describe_mismatches: Vec<String>,
    /// Duplicate ids, malformed ids, inconsistent hardware flags.
    pub id_problems: Vec<String>,
}

impl CoverageReport {
    /// No problem that must be fixed (`not_in_gui` is informational).
    pub fn ok(&self) -> bool {
        self.missing.is_empty()
            && self.required_missing.is_empty()
            && self.schema_errors.is_empty()
            && self.describe_mismatches.is_empty()
            && self.id_problems.is_empty()
    }

    /// Panic with the whole report when it is not `ok`. For tests.
    pub fn assert_ok(&self) {
        assert!(self.ok(), "MCP coverage problems:\n{}", serde_json::to_string_pretty(self).unwrap_or_default());
    }
}

fn valid_id(id: &str) -> bool {
    // `lang.<tag>`: the tag is a language code such as `zh-TW`.
    if let Some(tag) = id.strip_prefix("lang.") {
        return !tag.is_empty() && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    }
    id.contains('.')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
}

/// All commands, fetched uncompacted.
fn all(b: &mut dyn Backend) -> Result<Vec<CommandInfo>, CommandError> {
    Ok(b.list_commands(&ListQuery::default())?.commands)
}

/// Check the backend's registry against the GUI's action ids.
pub fn coverage_report<S: AsRef<str>>(backend: &mut dyn Backend, gui_actions: &[S]) -> CoverageReport {
    let mut r = CoverageReport::default();
    let cmds = match all(backend) {
        Ok(c) => c,
        Err(e) => {
            r.id_problems.push(format!("list_commands failed: {e}"));
            return r;
        }
    };
    r.registered = cmds.len();
    let ids: BTreeSet<&str> = cmds.iter().map(|c| c.id.as_str()).collect();
    let gui: BTreeSet<&str> = gui_actions.iter().map(AsRef::as_ref).collect();
    r.gui_actions = gui.len();
    r.missing = gui.iter().filter(|g| !ids.contains(**g)).map(|g| g.to_string()).collect();
    r.not_in_gui = ids.iter().filter(|i| !gui.contains(**i)).map(|i| i.to_string()).collect();
    r.required_missing = REQUIRED_COMMANDS.iter().filter(|i| !ids.contains(**i)).map(|i| i.to_string()).collect();
    if ids.len() != cmds.len() {
        let mut seen = BTreeSet::new();
        for c in &cmds {
            if !seen.insert(c.id.as_str()) {
                r.id_problems.push(format!("duplicate command id `{}`", c.id));
            }
        }
    }
    for c in &cmds {
        if !valid_id(&c.id) {
            r.id_problems.push(format!("`{}`: ids are dotted lower-case (a-z, 0-9, _), e.g. `edit.align_left`", c.id));
        }
        if c.hardware_class.is_some() && !c.hardware {
            r.id_problems
                .push(format!("`{}`: hardware_class is set but hardware is false (the gate would let it through as a normal command)", c.id));
        }
        if c.hardware_class == Some(HardwareClass::Stop) && !c.hardware {
            r.id_problems.push(format!("`{}`: a stop command must be flagged hardware", c.id));
        }
        for (what, s) in [("params_schema", &c.params_schema), ("result_schema", &c.result_schema)] {
            if s.is_null() {
                continue;
            }
            if !s.is_object() && !s.is_boolean() {
                r.schema_errors.push(format!("`{}`.{what}: must be a JSON object", c.id));
            } else if let Err(e) = check_schema_document(s) {
                r.schema_errors.push(format!("`{}`.{what}: not a valid JSON Schema: {e}", c.id));
            }
        }
        if c.params_schema.is_object() && c.params_schema["type"] != "object" {
            r.schema_errors.push(format!("`{}`.params_schema: the root must have \"type\": \"object\"", c.id));
        }
        match backend.describe_command(&c.id) {
            Ok(Some(d)) if d == *c => {}
            Ok(Some(_)) => r.describe_mismatches.push(format!("`{}`: describe_command differs from list_commands", c.id)),
            Ok(None) => r.describe_mismatches.push(format!("`{}`: listed but describe_command says unknown", c.id)),
            Err(e) => r.describe_mismatches.push(format!("`{}`: describe_command failed: {e}", c.id)),
        }
    }
    r
}

/// Fail (panic) unless every GUI action has a command and the registry is consistent.
pub fn assert_full_coverage<S: AsRef<str>>(backend: &mut dyn Backend, gui_actions: &[S]) {
    coverage_report(backend, gui_actions).assert_ok();
}
