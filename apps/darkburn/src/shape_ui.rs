//! Dialogs and ribbon controls of the shape-editing commands (offset, boolean assistant, close
//! with tolerance, auto-join, resize slots, arrays, nest, alignment helper). Every dialog builds
//! the JSON arguments of a registered command (`shape_ops.rs`) and runs it, so the dialog holds
//! no logic of its own; previews call the `*_preview` functions of `shape_ops`.

use darkburn_core::kurbo::BezPath;

/// The commands that open a dialog when run from a menu or button (headless callers pass
/// arguments instead and never see a dialog).
pub const DIALOG_COMMANDS: &[&str] = &[
    "tools.offset",
    "tools.boolean_assistant",
    "tools.resize_slots",
    "edit.close_tolerance",
    "edit.auto_join",
    "arrange.grid_array",
    "arrange.circular_array",
    "arrange.copy_along_path",
    "arrange.nest",
    "arrange.alignment_helper",
    "arrange.array_edit",
];

/// State of the shape dialogs and ribbon fields.
pub struct ShapeUi {
    /// Docking: move the whole selection as one block.
    pub dock_as_group: bool,
    /// Docking: shapes inside other selected shapes follow them.
    pub dock_lock_inner: bool,
    /// Radius (mm) of the Round Corners tool; negative rounds inwards.
    pub corner_radius: f64,
    /// The dialog that is open: the command id that opened it.
    pub open: Option<&'static str>,
    /// Preview outlines drawn over the canvas while a dialog is open (document mm).
    pub preview: Vec<BezPath>,
    /// What the last command reported ("Nodes 12 to 8", an error), for the status bar.
    pub status: Option<String>,
}

impl Default for ShapeUi {
    fn default() -> Self {
        ShapeUi { dock_as_group: false, dock_lock_inner: false, corner_radius: 1.0, open: None, preview: Vec::new(), status: None }
    }
}

impl ShapeUi {
    /// Open the dialog of command `id`.
    pub fn open_dialog(&mut self, id: &'static str) {
        self.open = Some(id);
    }
}

/// Draw the dialogs' preview outlines over the canvas.
pub fn paint_preview(app: &crate::shell::DarkBurn, xf: &crate::render::Xf, painter: &egui::Painter, accent: egui::Color32) {
    for path in &app.shape_ui.preview {
        for (mut pts, closed) in crate::render::screen_polylines(path, xf, true) {
            if closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, accent)));
        }
    }
}
