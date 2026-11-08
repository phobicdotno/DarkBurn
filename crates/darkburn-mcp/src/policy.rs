//! The policy the human controls from the app's settings. The app owns it; the gate reads it on
//! every call, so toggling a setting takes effect at once.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPolicy {
    /// "Allow AI agents to control machines". Off: every hardware command is denied (except
    /// emergency stop, pause and stop, which are always allowed).
    pub allow_hardware: bool,
    /// "Allow AI agents to write files" (save, export, machine files).
    pub allow_file_write: bool,
    /// Starting a job or firing the laser needs a one-time token issued after a human clicks
    /// Confirm in a dialog.
    pub require_confirmation_for_job_start: bool,
    /// Gweike M3 Ultra / MCC100 hardware commands, on top of `allow_hardware`.
    pub allow_gweike: bool,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self { allow_hardware: false, allow_file_write: true, require_confirmation_for_job_start: true, allow_gweike: false }
    }
}

/// Shared handle: the app keeps one clone to change the policy from its settings dialog, the gate
/// keeps another.
#[derive(Clone, Default)]
pub struct PolicyHandle(Arc<Mutex<McpPolicy>>);

impl PolicyHandle {
    pub fn new(policy: McpPolicy) -> Self {
        Self(Arc::new(Mutex::new(policy)))
    }
    pub fn get(&self) -> McpPolicy {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn set(&self, policy: McpPolicy) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = policy;
    }
    pub fn update(&self, f: impl FnOnce(&mut McpPolicy)) {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
