//! The canvas tools of the shape-editing commands: edit nodes, add tabs, measure, warp, deform,
//! start points, round corners, mirror line and two-point rotate and scale. Pointer handling
//! and overlays live here; every change goes through a registered command in `shape_ops.rs`,
//! `shape_nodes.rs` or `shape_arrays.rs` so it works without a window too.

use darkburn_core::kurbo::Point;
use egui::{Color32, Painter, Response, Ui};

use crate::render::Xf;
use crate::shell::DarkBurn;
use crate::tools::{Mods, Tool};

/// The status bar hint for `tool`.
pub fn hint(tool: Tool) -> &'static str {
    match tool {
        Tool::Nodes => "Edit nodes.",
        Tool::Tabs => "Click an outline to add a tab.",
        Tool::Measure => "Click two points to measure.",
        Tool::Warp => "Drag the four corners to warp the selection.",
        Tool::Deform => "Drag the 16 points to deform the selection.",
        Tool::StartPoints => "Click a node to make it the start.",
        Tool::RoundCorners => "Click a corner to round it.",
        Tool::Mirror => "Click two points on the mirror line.",
        Tool::TwoPoint => "Click the two source points, then the two target points.",
        _ => "",
    }
}

/// Handle the pointer for the active shape tool for one frame.
pub fn interact(_ui: &Ui, _app: &mut DarkBurn, _response: &Response, _xf: &Xf, _m: Mods, _raw: Option<Point>) {}

/// Draw the active tool's overlay.
pub fn paint(_ui: &Ui, _app: &DarkBurn, _xf: &Xf, _painter: &Painter, _accent: Color32, _dark: bool) {}
