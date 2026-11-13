//! Trace from a camera capture [IMG-20]. The Camera Control panel's Trace button takes a picture
//! now (from the running camera, or the picture file in use), lays it on the bed and opens the
//! Trace Image window on it. The picture is only there for the trace:
//!
//! - Cancel takes it away again and leaves no undo step behind;
//! - OK with "Delete image after tracing" (on by default here) leaves only the traced paths, as
//!   one undo step;
//! - OK with that box cleared keeps the picture as well (two undo steps: the picture, the trace);
//! - Capture again swaps in a new picture and keeps the window's settings.
//!
//! The design of these rules is DarkBurn's own; the notes only name the feature.

use darkburn_core::ShapeId;

use crate::image_ops::{self, TraceReport};
use crate::image_ui::TraceDialog;
use crate::shell::DarkBurn;

/// The undo label of the picture step (the one `vision_cmds::overlay_as_image` makes).
const PICTURE_STEP: &str = "Camera picture";

/// The camera picture a Trace Image window was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capture {
    pub id: ShapeId,
    /// Undo steps there were right after the picture was put in.
    steps: usize,
}

impl Capture {
    fn new(app: &DarkBurn, id: ShapeId) -> Self {
        Self { id, steps: app.editor.history.len() }
    }

    /// The picture is still the newest undo step, so it can be taken out as if it never came.
    fn still_last(self, app: &DarkBurn) -> bool {
        app.editor.history.len() == self.steps && app.editor.history.undo_label() == Some(PICTURE_STEP) && app.editor.doc.get(self.id).is_some()
    }
}

/// Take a picture and put it on the bed as an image shape.
fn take_picture(app: &mut DarkBurn) -> Result<ShapeId, String> {
    if app.vision.source.is_some() || app.vision.raw.is_some() {
        crate::vision_cmds::update_overlay(app)?;
    }
    crate::vision_cmds::overlay_as_image(app)
}

/// Trace: take a picture and open the Trace Image window on it.
pub fn open(app: &mut DarkBurn) -> Result<(), String> {
    let id = take_picture(app)?;
    let mut d = TraceDialog::new(app, vec![id])?;
    d.args.delete_image = true;
    d.capture = Some(Capture::new(app, id));
    app.images.trace = Some(d);
    Ok(())
}

/// Cancel or close: the picture goes again.
pub fn cancel(app: &mut DarkBurn, d: &TraceDialog) {
    if let Some(c) = d.capture
        && c.still_last(app)
    {
        app.editor.drop_checkpoint();
    }
}

/// OK: trace the picture.
pub fn finish(app: &mut DarkBurn, d: &TraceDialog) -> Result<TraceReport, String> {
    let args = d.effective();
    match d.capture {
        Some(c) if args.delete_image && c.still_last(app) => {
            let paths = image_ops::trace_paths(&app.editor.doc, c.id, &args)?;
            if paths.is_empty() {
                return Err("nothing was found to trace: try a higher threshold".into());
            }
            let layer = image_ops::trace_layer(&app.editor.doc, args.layer, app.editor.current_layer);
            app.editor.drop_checkpoint();
            image_ops::add_traced_step(&mut app.editor, "Trace camera picture", paths, layer)
        }
        _ => image_ops::trace(&mut app.editor, &d.ids, &args),
    }
}

/// Capture again: a new picture in place of the one the window shows.
pub fn again(app: &mut DarkBurn, d: &mut TraceDialog) -> Result<(), String> {
    let Some(c) = d.capture else { return Err("this window is not on a camera picture".into()) };
    if app.vision.source.is_none() {
        return Err("Start the camera (or open a picture file) to take another picture.".into());
    }
    if c.still_last(app) {
        app.editor.drop_checkpoint();
    }
    let id = take_picture(app)?;
    d.ids = vec![id];
    d.capture = Some(Capture::new(app, id));
    d.reset_preview();
    Ok(())
}
