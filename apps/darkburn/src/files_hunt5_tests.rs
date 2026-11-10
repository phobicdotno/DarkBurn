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

/// Kerf, Z offset and air assist of a layer come back from a LightBurn file the app wrote
/// itself: the reader knows them, so the writer has to write them.
#[test]
fn a_lightburn_file_keeps_kerf_z_offset_and_air() {
    let dir = TempDir::new("lbrnlayer");
    let mut a = app();
    run(&mut a, "shape.add_rect", json!({ "x": 20, "y": 30, "width": 40, "height": 20, "layer": 1 })).unwrap();
    let air = !a.editor.doc.layer(1).air;
    {
        let l = a.editor.doc.layer_mut(1);
        l.air = air;
        let m = l.main_mut();
        m.kerf = 0.15;
        m.z_offset = 2.5;
    }
    let file = dir.file("layer.lbrn2");
    run(&mut a, "project.export", json!({ "path": file })).unwrap();
    let mut b = app();
    run(&mut b, "project.open", json!({ "path": file })).unwrap();
    let l = b.editor.doc.layer(1);
    assert_eq!((l.main().kerf, l.main().z_offset), (0.15, 2.5));
    assert_eq!(l.air, air, "air assist");
}

/// A LightBurn file only says a text's words, font, height and side alignment. Its other
/// settings are in the ready-made letters saved with it, so a text opened from the file keeps
/// those letters until it is changed.
#[test]
fn styled_text_opened_from_a_lightburn_file_keeps_its_letters() {
    let dir = TempDir::new("lbrnstyle");
    let mut a = app();
    let id = run(&mut a, "text.add", json!({ "text": "Wide", "x": 50, "y": 50, "height": 10, "font": "IBM Plex Sans", "layer": 0 })).unwrap()["id"]
        .as_u64()
        .unwrap();
    if let ShapeKind::Text(t) = &mut a.editor.doc.get_mut(ShapeId(id)).unwrap().kind {
        t.letter_spacing = 4.0;
        t.bold = true;
        t.align_y = darkburn_core::shape::AlignY::Top;
    }
    let before = crate::text_render::prepare_for_output(&a.editor.doc).bbox().unwrap();
    let file = dir.file("style.lbrn2");
    run(&mut a, "project.export", json!({ "path": file })).unwrap();
    let mut b = app();
    run(&mut b, "project.open", json!({ "path": file })).unwrap();
    let built = crate::text_render::prepare_for_output(&b.editor.doc).bbox().unwrap();
    assert!(near(built, before, 0.05), "as burned: {built:?} vs {before:?}");
}

/// A cut library file is read like a project: numbers no editor field could hold (power above
/// 100 %, no speed at all, more sub-layers than a layer has) never reach a layer.
#[test]
fn a_cut_library_file_cannot_put_impossible_settings_on_a_layer() {
    use crate::library::{Entry, Library, assign};
    use darkburn_core::{CutSetting, MAX_SUB_LAYERS};
    let wild = CutSetting { max_power: 500.0, min_power: -20.0, speed: 0.0, passes: 0, interval: 0.0, ..Default::default() };
    let lib = Library { entries: vec![Entry { material: "Birch".into(), settings: vec![wild; 40], ..Default::default() }], ..Default::default() };
    let back = Library::from_json(&lib.to_json()).unwrap();
    let mut a = app();
    assign(&mut a.editor.doc, 1, &back.entries[0]).unwrap();
    let l = a.editor.doc.layer(1);
    assert!(l.settings.len() <= MAX_SUB_LAYERS, "{} settings on one layer", l.settings.len());
    let m = l.main();
    assert_eq!((m.max_power, m.min_power, m.passes), (100.0, 0.0, 1));
    assert!(m.speed > 0.0 && m.interval > 0.0, "speed {} interval {}", m.speed, m.interval);
}
