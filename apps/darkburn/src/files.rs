//! Project files: new, open, save, recent projects, notes, auto-save and crash restore, and the
//! system clipboard.

use crate::shell::DarkBurn;

/// Run a File command; returns whether `id` was one.
pub fn run(_app: &mut DarkBurn, _ctx: &egui::Context, _id: &str) -> bool {
    false
}

/// Windows owned by this module (notes, import options, questions).
pub fn dialogs(_ctx: &egui::Context, _app: &mut DarkBurn) {}

/// Before a paste: take shapes from the system clipboard when it holds something newer.
pub fn pull_system_clipboard(_app: &mut DarkBurn, _ctx: &egui::Context) {}

/// Once a frame: auto-save, push copies to the system clipboard, take dropped files.
pub fn frame(_ctx: &egui::Context, _app: &mut DarkBurn) {}
