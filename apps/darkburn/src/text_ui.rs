//! The text interface: the text tool (place, type in place, double-click to edit), the Edit
//! Text Shapes dialog, the font picker, the QR code dialog and the Variable Text panel.
//! Every change goes through the commands in `text_cmds.rs`.

use darkburn_core::kurbo::Point;
use darkburn_core::{Editor, ShapeId};
use egui::{Context, Response, Ui};

use crate::shell::DarkBurn;
use crate::text_render as render;
use crate::render::Xf;

pub const HINT: &str = "Click to place text and type. Enter starts a new line, Esc or a click elsewhere finishes. Double-click text to edit it.";

/// The QR code dialog.
#[derive(Default)]
pub struct QrDialog {
    pub open: bool,
}

impl QrDialog {
    pub fn open_new(&mut self) {
        self.open = true;
    }
}

/// State of the text interface.
#[derive(Default)]
pub struct TextUi {
    pub qr: QrDialog,
    pub font_picker_open: bool,
    pub edit_open: bool,
    /// The editor revision the document was last rendered at, and when (egui seconds).
    rendered_rev: Option<u64>,
    rendered_at: f64,
}

impl TextUi {
    pub fn open_edit_dialog(&mut self, _editor: &Editor) {
        self.edit_open = true;
    }
}

/// Once a frame: render text and codes whose inputs changed (undo, array edits, new
/// variable text state, the clock for date and time text).
pub fn tick(app: &mut DarkBurn, ctx: &Context) {
    let now = ctx.input(|i| i.time);
    let rev = app.editor.revision();
    if app.text.rendered_rev == Some(rev) && now - app.text.rendered_at < 1.0 {
        return;
    }
    app.text.rendered_rev = Some(rev);
    app.text.rendered_at = now;
    let was_modified = app.editor.is_modified();
    if render::refresh(&mut app.editor.doc) > 0 {
        app.editor.touch();
        app.text.rendered_rev = Some(app.editor.revision());
        if !was_modified {
            app.editor.mark_saved();
        }
    }
}

/// The text tool on the canvas.
pub fn interact(_ui: &Ui, _app: &mut DarkBurn, _response: &Response, _xf: &Xf, _raw: Option<Point>) {}

#[allow(dead_code)]
fn _unused(_: ShapeId) {}
