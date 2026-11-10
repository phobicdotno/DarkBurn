//! Bug hunt pass 5, files: project files, import and export through the command layer.

use std::path::PathBuf;

use darkburn_core::kurbo::{Affine, Rect};
use darkburn_core::{ShapeId, ShapeKind};
use serde_json::{Value, json};

use crate::DarkBurn;
use crate::api::execute_headless;
use crate::prefs::Prefs;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let d = std::env::temp_dir().join(format!("darkburn-hunt5-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn near(a: Rect, b: Rect, tol: f64) -> bool {
    (a.x0 - b.x0).abs() < tol && (a.y0 - b.y0).abs() < tol && (a.x1 - b.x1).abs() < tol && (a.y1 - b.y1).abs() < tol
}

/// A text saved to a LightBurn file and opened again must still be where it was when the job
/// is built (the job builder renders every text again first).
#[test]
fn text_opened_from_a_lightburn_file_stays_in_place_when_the_job_is_built() {
    let dir = TempDir::new("lbrntext");
    let mut a = app();
    let id = run(&mut a, "text.add", json!({ "text": "Hello", "x": 0, "y": 0, "height": 10, "font": "IBM Plex Sans", "layer": 0 })).unwrap()["id"]
        .as_u64()
        .unwrap();
    a.editor.doc.get_mut(ShapeId(id)).unwrap().transform = Affine::translate((120.0, 80.0)) * Affine::rotate(0.5);
    let before = crate::text_render::prepare_for_output(&a.editor.doc).bbox().unwrap();
    assert!(before.width() > 10.0 && before.x0 > 90.0, "the text has letters and sits away from the corner: {before:?}");
    let file = dir.file("text.lbrn2");
    run(&mut a, "project.export", json!({ "path": file })).unwrap();

    let mut b = app();
    run(&mut b, "project.open", json!({ "path": file })).unwrap();
    assert!(b.editor.doc.iter().any(|s| matches!(s.kind, ShapeKind::Text(_))), "the text is still a text");
    let opened = b.editor.doc.bbox().unwrap();
    assert!(near(opened, before, 0.05), "as opened: {opened:?} vs {before:?}");
    let built = crate::text_render::prepare_for_output(&b.editor.doc).bbox().unwrap();
    assert!(near(built, before, 0.05), "as burned: {built:?} vs {before:?}");
}

/// A Fill+Line layer comes back as Fill+Line from a LightBurn file the app wrote itself.
#[test]
fn a_lightburn_file_keeps_a_fill_and_line_layer() {
    use darkburn_core::layer::Mode;
    let dir = TempDir::new("lbrnmode");
    let mut a = app();
    run(&mut a, "shape.add_rect", json!({ "x": 20, "y": 30, "width": 40, "height": 20, "layer": 1 })).unwrap();
    a.editor.doc.layer_mut(1).main_mut().mode = Mode::FillLine;
    let file = dir.file("layer.lbrn2");
    run(&mut a, "project.export", json!({ "path": file })).unwrap();
    let mut b = app();
    run(&mut b, "project.open", json!({ "path": file })).unwrap();
    assert_eq!(b.editor.doc.layer(1).main().mode, Mode::FillLine, "Fill+Line stays Fill+Line");
}
