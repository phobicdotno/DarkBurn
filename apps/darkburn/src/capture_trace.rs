//! Trace from a camera capture [IMG-20]. The Camera Control panel's Trace button runs the command
//! `camera.trace_picture`: it takes a picture now (from the running camera, or the picture file in
//! use; for a head camera with captures, the stitched picture), lays it on the bed and opens the
//! Trace Image window on it. The picture is only there for the trace:
//!
//! - Cancel takes it away again and leaves no undo step behind, and what could be redone before
//!   still can;
//! - OK with "Delete image after tracing" (on by default here) leaves only the traced paths, as
//!   one undo step;
//! - OK with that box cleared keeps the picture as well (two undo steps: the picture, the trace);
//! - Capture again swaps in a new picture and keeps the window's settings. When no new picture
//!   can be taken, the one the window shows stays. Trace in the panel while the window is open
//!   does the same.
//!
//! With `apply`, the command traces at once with the settings it is given, as one undo step, so
//! an agent can do what the window does.
//!
//! The design of these rules is DarkBurn's own; the notes only name the feature.

use darkburn_core::{RedoSteps, ShapeId};
use darkburn_vision::camera::CameraKind;
use serde_json::{Value, json};

use crate::image_ops::{self, TraceArgs, TraceReport};
use crate::image_ui::TraceDialog;
use crate::shell::DarkBurn;
use crate::vision_cmds::{self, OverlayPicture};

/// The undo label of the picture step (the one `vision_cmds::add_overlay_picture` makes).
const PICTURE_STEP: &str = "Camera picture";

/// The camera picture a Trace Image window was opened on.
#[derive(Debug)]
pub struct Capture {
    pub id: ShapeId,
    /// Undo steps there were right after the picture was put in.
    steps: usize,
    /// What could be redone before the picture came; Cancel puts it back.
    redo: RedoSteps,
}

impl Capture {
    /// The picture is still the newest undo step, so it can be taken out as if it never came.
    fn still_last(&self, app: &DarkBurn) -> bool {
        app.editor.history.len() == self.steps && app.editor.history.undo_label() == Some(PICTURE_STEP) && app.editor.doc.get(self.id).is_some()
    }
}

/// A head camera with captures: its picture is the one Stitch made, not a new frame.
fn stitched(app: &DarkBurn) -> bool {
    app.vision.state.camera.kind == CameraKind::HeadMounted && !app.vision.mosaic.is_empty()
}

/// Make a new picture ready without touching the document. A running camera is asked for a new
/// picture now rather than the one the viewfinder last showed. A head camera with captures gives
/// the stitched picture.
fn fresh_picture(app: &mut DarkBurn) -> Result<OverlayPicture, String> {
    if stitched(app) {
        if app.vision.overlay.is_none() {
            return Err("press Stitch in Head Camera Alignment first".into());
        }
    } else {
        if app.vision.live
            && let Some(src) = app.vision.source.as_mut()
        {
            let img = src.grab().map_err(|e| e.to_string())?;
            app.vision.raw = Some(img);
            app.vision.frame_gen += 1;
        }
        if app.vision.source.is_some() || app.vision.raw.is_some() {
            vision_cmds::update_overlay(app)?;
        }
    }
    vision_cmds::overlay_picture(app)
}

/// Put the picture in, keeping the redo steps there were for Cancel to give back.
fn put_in(app: &mut DarkBurn, pic: OverlayPicture, redo: RedoSteps) -> Capture {
    let id = vision_cmds::add_overlay_picture(app, pic);
    Capture { id, steps: app.editor.history.len(), redo }
}

fn camera_window_open(app: &DarkBurn) -> bool {
    app.images.trace.as_ref().is_some_and(|d| d.capture.is_some())
}

/// Trace: take a picture and open the Trace Image window on it. When the window is already open
/// on a camera picture, a new picture takes that one's place (as Capture again).
pub fn open(app: &mut DarkBurn) -> Result<(), String> {
    if camera_window_open(app) {
        let mut d = app.images.trace.take().expect("checked above");
        let r = again(app, &mut d);
        app.images.trace = Some(d);
        return r;
    }
    let pic = fresh_picture(app)?;
    let redo = app.editor.history.take_redo();
    let c = put_in(app, pic, redo);
    let mut d = match TraceDialog::new(app, vec![c.id]) {
        Ok(d) => d,
        Err(e) => {
            app.editor.drop_checkpoint();
            app.editor.history.put_back_redo(c.redo);
            return Err(e);
        }
    };
    d.args.delete_image = true;
    d.capture = Some(c);
    app.images.trace = Some(d);
    Ok(())
}

/// Cancel or close: the picture goes again, and what could be redone before still can.
pub fn cancel(app: &mut DarkBurn, d: &mut TraceDialog) {
    if let Some(c) = d.capture.take()
        && c.still_last(app)
    {
        app.editor.drop_checkpoint();
        app.editor.history.put_back_redo(c.redo);
    }
}

/// OK: trace the picture.
pub fn finish(app: &mut DarkBurn, d: &TraceDialog) -> Result<TraceReport, String> {
    let args = d.effective();
    match &d.capture {
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

/// Capture again: a new picture in place of the one the window shows. The new picture is taken
/// first, so when that fails the old one stays and the window can still trace it.
pub fn again(app: &mut DarkBurn, d: &mut TraceDialog) -> Result<(), String> {
    if d.capture.is_none() {
        return Err("this window is not on a camera picture".into());
    }
    if app.vision.source.is_none() && !stitched(app) {
        return Err("Start the camera (or open a picture file) to take another picture.".into());
    }
    let pic = fresh_picture(app)?;
    let old = d.capture.take().expect("checked above");
    let redo = if old.still_last(app) {
        app.editor.drop_checkpoint();
        old.redo
    } else {
        // Something was changed after the picture came: the old picture stays where it is.
        app.editor.history.take_redo()
    };
    let c = put_in(app, pic, redo);
    d.ids = vec![c.id];
    d.capture = Some(c);
    d.reset_preview();
    Ok(())
}

/// Set the window's settings from command arguments.
fn set_args(d: &mut TraceDialog, args: TraceArgs) {
    if let Some([x0, y0, x1, y1]) = args.clip {
        d.clip_on = true;
        d.clip = [x0 * 100.0, y0 * 100.0, x1 * 100.0, y1 * 100.0];
    }
    d.args = args;
}

/// The command `camera.trace_picture`: open Trace Image on a fresh picture, or with `apply` trace
/// it at once. Trace settings are optional; `delete_image` is on unless set false.
pub fn command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let apply = a.get("apply").and_then(Value::as_bool).unwrap_or(false);
    let mut settings = a.clone();
    if let Some(o) = settings.as_object_mut() {
        o.remove("apply");
    }
    let mut args: TraceArgs = if settings.as_object().is_none_or(|o| o.is_empty()) {
        TraceArgs::default()
    } else {
        serde_json::from_value(settings).map_err(|e| format!("trace settings: {e}"))?
    };
    if a.get("delete_image").is_none() {
        args.delete_image = true;
    }
    open(app)?;
    let mut d = app.images.trace.take().ok_or("Trace Image did not open")?;
    set_args(&mut d, args);
    let id = d.capture.as_ref().map(|c| c.id.0);
    if !apply {
        app.images.trace = Some(d);
        return Ok(json!({ "opened": "trace_image", "picture": id }));
    }
    match finish(app, &d) {
        Ok(r) => Ok(json!({ "created": r.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "nodes": r.nodes })),
        Err(e) => {
            cancel(app, &mut d);
            Err(e)
        }
    }
}
