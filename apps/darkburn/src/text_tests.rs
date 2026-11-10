//! Headless tests of the text commands: text, text on a path, QR codes, barcodes, variable text,
//! the project file and the job.

use darkburn_core::kurbo::{Rect, Shape as _};
use darkburn_core::shape::VariableText;
use darkburn_core::{CodeSpec, Document, Shape, ShapeId, ShapeKind, TextShape};
use darkburn_text::{EccLevel, EvalContext};
use serde_json::{Value, json};

use crate::shape_ops::tests::{app, run, select};
use crate::shell::DarkBurn;
use crate::text_render as render;

fn add(a: &mut DarkBurn, text: &str, extra: Value) -> ShapeId {
    let mut args = json!({ "text": text, "x": 10, "y": 10, "height": 10, "font": "IBM Plex Sans", "layer": 0 });
    for (k, v) in extra.as_object().unwrap() {
        args[k] = v.clone();
    }
    ShapeId(run(a, "text.add", args).unwrap()["id"].as_u64().unwrap())
}

fn bbox(a: &DarkBurn, id: ShapeId) -> Rect {
    a.editor.doc.get(id).unwrap().bbox()
}

fn text_of(a: &DarkBurn, id: ShapeId) -> TextShape {
    match &a.editor.doc.get(id).unwrap().kind {
        ShapeKind::Text(t) => t.clone(),
        _ => panic!("not text"),
    }
}

fn code_key(a: &DarkBurn, id: ShapeId) -> u64 {
    match &a.editor.doc.get(id).unwrap().kind {
        ShapeKind::Code(c) => c.render_key,
        _ => 0,
    }
}

#[test]
fn an_shx_font_file_is_added_and_draws_single_line_text() {
    use darkburn_text::shx::write_shapes_1_0;
    let dir = std::env::temp_dir().join(format!("darkburn-shx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let l = vec![0x84, 2, 0x8C, 1, 0x40, 2, 0x20, 0];
    let path = dir.join("test.shx");
    std::fs::write(&path, write_shapes_1_0("TESTFONT", 8, 2, &[(b'L' as u16, "L", l)])).unwrap();
    let mut a = app();
    let out = run(&mut a, "text.add_shx", json!({ "path": path.display().to_string() })).unwrap();
    assert_eq!(out["glyphs"], 1);
    let font = out["font"].as_str().unwrap().to_string();
    assert!(a.prefs.shx_fonts.contains(&font), "listed in the font picker");
    let id = add(&mut a, "L", json!({ "font": font }));
    let bb = bbox(&a, id);
    assert!(bb.height() > 5.0, "the L has height: {bb:?}");
    assert!(run(&mut a, "text.add_shx", json!({ "path": dir.join("missing.shx").display().to_string() })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn text_has_real_outlines_with_the_asked_height() {
    let mut a = app();
    let id = add(&mut a, "HEH", json!({}));
    let bb = bbox(&a, id);
    assert!(bb.width() > 10.0 && bb.width() < 40.0, "{bb:?}");
    assert!((bb.height() - 10.0).abs() < 0.5, "capital height is the asked height: {bb:?}");
    assert!((bb.y0 - 10.0).abs() < 0.2, "baseline sits on the click point: {bb:?}");
    assert!(text_of(&a, id).backup_glyphs.is_some(), "the glyphs are kept for machines without the font");
}

#[test]
fn text_settings_change_the_outline_and_undo_in_one_step() {
    let mut a = app();
    let id = add(&mut a, "Ab", json!({}));
    let w0 = bbox(&a, id).width();
    run(&mut a, "text.set", json!({ "ids": [id.0], "letter_spacing": 5.0 })).unwrap();
    assert!(bbox(&a, id).width() > w0 + 4.0);
    run(&mut a, "text.set", json!({ "ids": [id.0], "letter_spacing": 0.0, "text": "Ab\nAb" })).unwrap();
    let two = bbox(&a, id);
    assert!(two.height() > 15.0, "two lines: {two:?}");
    run(&mut a, "text.set", json!({ "ids": [id.0], "text": "Ab", "align_x": "middle", "align_y": "middle" })).unwrap();
    let bb = bbox(&a, id);
    assert!((bb.center().x - 10.0).abs() < 1.5 && (bb.center().y - 10.0).abs() < 1.5, "centred on the click point: {bb:?}");
    run(&mut a, "text.set", json!({ "ids": [id.0], "upper_case": true, "text": "ac" })).unwrap();
    let upper = bbox(&a, id);
    run(&mut a, "text.set", json!({ "ids": [id.0], "upper_case": false })).unwrap();
    assert!(upper.height() > bbox(&a, id).height(), "lower case ac is shorter than AC");
    let before = a.editor.history.len();
    run(&mut a, "text.set", json!({ "ids": [id.0], "bold": true })).unwrap();
    assert_eq!(a.editor.history.len(), before + 1);
    run(&mut a, "edit.undo", Value::Null).unwrap();
    assert!(!text_of(&a, id).bold);
}

#[test]
fn several_text_objects_are_edited_at_once() {
    let mut a = app();
    let (t1, t2) = (add(&mut a, "One", json!({})), add(&mut a, "Two", json!({ "y": 40 })));
    select(&mut a, &[t1, t2]);
    run(&mut a, "text.set", json!({ "height": 5.0 })).unwrap();
    assert_eq!(text_of(&a, t1).height, 5.0);
    assert_eq!(text_of(&a, t2).height, 5.0);
    assert!(bbox(&a, t1).height() < 6.0);
}

#[test]
fn right_to_left_text_and_welding() {
    let mut a = app();
    let id = add(&mut a, "\u{5d0}\u{5d1}\u{5d2} abc", json!({}));
    assert!(a.editor.doc.get(id).is_some());
    let weld = add(&mut a, "ll", json!({ "welded": false, "letter_spacing": -2.5 }));
    let n_plain = a.editor.doc.get(weld).unwrap().outline().elements().len();
    run(&mut a, "text.set", json!({ "ids": [weld.0], "welded": true })).unwrap();
    let n_weld = a.editor.doc.get(weld).unwrap().outline().elements().len();
    assert_ne!(n_plain, n_weld, "overlapping letters merge");
}

#[test]
fn text_flows_into_the_job() {
    let mut a = app();
    add(&mut a, "CUT", json!({}));
    let r = run(&mut a, "job.generate", Value::Null).unwrap();
    assert!(r["cut_mm"].as_f64().unwrap() > 40.0, "{r}");
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap().contains("no outline")), "{r}");
}

#[test]
fn project_round_trip_keeps_text_and_backup_outlines() {
    let mut a = app();
    let id = add(&mut a, "Keep me", json!({ "font": "A Font Nobody Has", "bold": true }));
    run(&mut a, "variable.set", json!({ "current": 7, "auto_advance": true })).unwrap();
    let outline = a.editor.doc.get(id).unwrap().outline();
    let bytes = darkburn_core::project::save(&a.editor.doc, "test", None).unwrap();
    let loaded = darkburn_core::project::load(&bytes).unwrap();
    assert!(!loaded.newer_version);
    assert_eq!(loaded.doc.variables.serial.current, 7);
    assert!(loaded.doc.variables.auto_advance);
    let t = match &loaded.doc.get(id).unwrap().kind {
        ShapeKind::Text(t) => t.clone(),
        _ => panic!(),
    };
    assert!(t.backup_glyphs.is_some() && t.backup.is_some());
    assert_eq!(loaded.doc.get(id).unwrap().outline().elements().len(), outline.elements().len(), "same outline after the round trip");
    // The font is missing here: rendering again uses the backup glyphs and gives the same shape.
    let mut doc = loaded.doc.clone();
    if let Some(s) = doc.get_mut(id)
        && let ShapeKind::Text(t) = &mut s.kind
    {
        t.render_key = 0;
    }
    render::refresh(&mut doc);
    assert_eq!(doc.get(id).unwrap().outline().elements().len(), outline.elements().len());
}

#[test]
fn old_project_files_still_load() {
    use std::io::{Read, Write};
    let mut d = Document::new();
    d.add(Shape::rect(Rect::new(0.0, 0.0, 5.0, 5.0), 0.0, 0));
    let bytes = darkburn_core::project::save(&d, "old", None).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut json = String::new();
    z.by_name("project.json").unwrap().read_to_string(&mut json).unwrap();
    let mut v: Value = serde_json::from_str(&json).unwrap();
    v["version"] = json!(1);
    v.as_object_mut().unwrap().remove("variables");
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    out.start_file("project.json", zip::write::SimpleFileOptions::default()).unwrap();
    out.write_all(v.to_string().as_bytes()).unwrap();
    let old = out.finish().unwrap().into_inner();
    let loaded = darkburn_core::project::load(&old).unwrap();
    assert_eq!(loaded.doc.len(), 1);
    assert_eq!(loaded.doc.variables.serial.current, 1);
}

#[test]
fn text_on_a_path_and_apply_path_to_text() {
    let mut a = app();
    let t = add(&mut a, "ROUND THE BEND", json!({ "height": 5, "x": 0, "y": 0 }));
    let flat = bbox(&a, t);
    let p = ShapeId(run(&mut a, "shape.add_path", json!({ "svg_path": "M 0 0 C 30 40 60 40 90 0", "layer": 1 })).unwrap()["id"].as_u64().unwrap());
    select(&mut a, &[t, p]);
    run(&mut a, "tools.path_to_text", Value::Null).unwrap();
    let bent = bbox(&a, t);
    assert!(bent.height() > flat.height() + 5.0, "the text rises with the path: {flat:?} {bent:?}");
    assert!(text_of(&a, t).on_path.is_some());
    run(&mut a, "text.set", json!({ "ids": [t.0], "distort": true })).unwrap();
    assert!(bbox(&a, t).height() > flat.height());
    run(&mut a, "text.detach_path", json!({ "ids": [t.0] })).unwrap();
    assert!((bbox(&a, t).height() - flat.height()).abs() < 0.1);
}

#[test]
fn qr_codes_stay_editable() {
    let mut a = app();
    let r = run(&mut a, "qr.add", json!({ "kind": "url", "url": "example.com", "x": 50, "y": 50, "size": 20 })).unwrap();
    let id = ShapeId(r["id"].as_u64().unwrap());
    let bb = bbox(&a, id);
    assert!((bb.width() - 20.0).abs() < 0.1, "size is the symbol side: {bb:?}");
    assert!((bb.center().x - 50.0).abs() < 0.1);
    let cut_before = run(&mut a, "job.generate", Value::Null).unwrap()["cut_mm"].as_f64().unwrap();
    assert!(cut_before > 20.0);
    run(&mut a, "qr.set", json!({ "ids": [id.0], "kind": "wifi", "ssid": "Home", "password": "secret", "ecc": "H" })).unwrap();
    match &a.editor.doc.get(id).unwrap().kind {
        ShapeKind::Code(c) => match &c.spec {
            CodeSpec::Qr(q) => {
                assert_eq!(q.ecc, EccLevel::H);
                assert_eq!(q.content.payload(), "WIFI:T:WPA;S:Home;P:secret;;");
            }
            _ => panic!(),
        },
        _ => panic!(),
    }
    let r = run(&mut a, "qr.set", json!({ "ids": [id.0], "kind": "text", "text": "x".repeat(2500) })).unwrap();
    assert!(r["errors"][0].as_str().unwrap().contains("2000"), "{r}");
    assert!(a.editor.doc.get(id).unwrap().outline().elements().is_empty(), "a code that cannot be made burns nothing");
}

#[test]
fn every_barcode_kind_is_made() {
    let mut a = app();
    for (k, d) in [
        ("code128", "Hello-123"),
        ("code39", "ABC-123"),
        ("ean13", "590123412345"),
        ("upca", "03600029145"),
        ("itf", "123456"),
        ("datamatrix", "DarkBurn"),
    ] {
        let r = run(&mut a, "barcode.add", json!({ "barcode": k, "data": d })).unwrap();
        assert!(r["errors"].as_array().unwrap().is_empty(), "{k}: {r}");
        let id = ShapeId(r["id"].as_u64().unwrap());
        assert!(bbox(&a, id).width() > 3.0, "{k}");
    }
    let r = run(&mut a, "barcode.add", json!({ "barcode": "ean13", "data": "abc" })).unwrap();
    assert!(!r["errors"].as_array().unwrap().is_empty());
}

#[test]
fn serial_numbers_and_navigation() {
    let mut a = app();
    let id = add(&mut a, "SN{serial:3}", json!({ "variable": "serial" }));
    run(&mut a, "variable.set", json!({ "start": 1, "end": 12, "current": 9, "advance_by": 1 })).unwrap();
    assert_eq!(run(&mut a, "variable.test", json!({ "count": 3 })).unwrap()["values"], json!(["SN009", "SN010", "SN011"]));
    run(&mut a, "variable.next", Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 10);
    run(&mut a, "variable.previous", Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 9);
    run(&mut a, "variable.reset", Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 1);
    run(&mut a, "variable.set", json!({ "current": 5 })).unwrap();
    select(&mut a, &[id]);
    run(&mut a, "variable.bake", Value::Null).unwrap();
    let t = text_of(&a, id);
    assert_eq!(t.text, "SN005");
    assert_eq!(t.variable, VariableText::Normal);
    run(&mut a, "variable.next", Value::Null).unwrap();
    assert_eq!(text_of(&a, id).text, "SN005", "baked text no longer follows the index");
}

#[test]
fn csv_merge_and_qr_follow_the_current_row() {
    let mut a = app();
    let dir = std::env::temp_dir().join(format!("darkburn-vt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("names.csv");
    std::fs::write(&file, "name,code\n\"Smith, Ann\",A1\nBob,B22\n\"Multi\nline\",C333\n").unwrap();
    let r = run(&mut a, "variable.csv_load", json!({ "path": file.to_string_lossy(), "header": true })).unwrap();
    assert_eq!(r["csv"]["rows"], 3);
    let t = add(&mut a, "{csv:code}", json!({ "variable": "merge" }));
    let q = ShapeId(run(&mut a, "qr.add", json!({ "kind": "text", "text": "{csv:name}", "variable": true })).unwrap()["id"].as_u64().unwrap());
    let w_a1 = bbox(&a, t).width();
    let key_a = code_key(&a, q);
    run(&mut a, "variable.next", Value::Null).unwrap();
    run(&mut a, "variable.next", Value::Null).unwrap();
    assert!(bbox(&a, t).width() > w_a1, "C333 is wider than A1");
    assert_ne!(key_a, code_key(&a, q), "the QR code follows the row");
    run(&mut a, "variable.csv_clear", Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.merge.row_count(), 0);
}

#[test]
fn variable_text_works_inside_arrays() {
    let mut a = app();
    let id = add(&mut a, "{serial:2}", json!({ "variable": "serial", "x": 0, "y": 0 }));
    select(&mut a, &[id]);
    run(&mut a, "arrange.grid_array", json!({ "rows": 1, "cols": 3, "dx": 5, "dy": 5, "virtual": true })).unwrap();
    render::refresh(&mut a.editor.doc);
    let arr = a.editor.doc.iter().next().unwrap().clone();
    let ShapeKind::Group { children } = &arr.kind else { panic!("{:?}", arr.kind) };
    let offsets: Vec<i64> = children.iter().filter_map(|c| if let ShapeKind::Text(t) = &c.kind { Some(t.offset) } else { None }).collect();
    assert_eq!(offsets, [0, 1, 2], "each copy has its own index");
    let keys: Vec<u64> = children.iter().filter_map(|c| if let ShapeKind::Text(t) = &c.kind { Some(t.render_key) } else { None }).collect();
    assert!(keys[0] != keys[1] && keys[1] != keys[2], "each copy shows its own number");
    let r = run(&mut a, "job.generate", Value::Null).unwrap();
    assert!(r["cut_mm"].as_f64().unwrap() > 20.0);
}

#[test]
fn variable_text_counts_up_in_real_arrays_too() {
    for (cmd, args) in [
        ("arrange.grid_array", json!({ "rows": 1, "cols": 3, "dx": 5, "dy": 5 })),
        ("arrange.circular_array", json!({ "count": 3, "cx": 50, "cy": 50 })),
    ] {
        let mut a = app();
        let id = add(&mut a, "{serial:2}", json!({ "variable": "serial", "x": 0, "y": 0 }));
        select(&mut a, &[id]);
        run(&mut a, cmd, args).unwrap();
        let mut offsets: Vec<i64> =
            a.editor.doc.iter().filter_map(|s| if let ShapeKind::Text(t) = &s.kind { Some(t.offset) } else { None }).collect();
        offsets.sort();
        assert_eq!(offsets, [0, 1, 2], "{cmd}: each copy has its own index");
    }
}

#[test]
fn date_time_text_is_evaluated_for_output() {
    let mut a = app();
    let id = add(&mut a, "%Y", json!({ "variable": "date_time" }));
    let doc = render::prepare_for_output(&a.editor.doc);
    let t = match &doc.get(id).unwrap().kind {
        ShapeKind::Text(t) => t.clone(),
        _ => panic!(),
    };
    let ctx = EvalContext::local_now();
    let year = darkburn_text::vartext::format_time(&ctx.now, "%Y");
    assert_eq!(render::evaluated_text(&doc, 0, &t, &ctx), year);
    assert!(doc.get(id).unwrap().outline().bounding_box().width() > 5.0);
}

#[test]
fn auto_advance_moves_on_after_a_job_only_with_variable_text() {
    let mut a = app();
    run(&mut a, "variable.set", json!({ "current": 1, "auto_advance": true })).unwrap();
    add(&mut a, "plain", json!({}));
    assert!(!run(&mut a, "variable.after_job", Value::Null).unwrap()["advanced"].as_bool().unwrap());
    add(&mut a, "{serial}", json!({ "variable": "serial" }));
    assert!(run(&mut a, "variable.after_job", Value::Null).unwrap()["advanced"].as_bool().unwrap());
    assert_eq!(a.editor.doc.variables.serial.current, 2);
}

#[test]
fn convert_to_path_bakes_variable_text_and_codes() {
    let mut a = app();
    let id = add(&mut a, "{serial:3}", json!({ "variable": "serial" }));
    run(&mut a, "variable.set", json!({ "current": 42 })).unwrap();
    let outline = a.editor.doc.get(id).unwrap().outline();
    select(&mut a, &[id]);
    run(&mut a, "edit.convert_path", Value::Null).unwrap();
    let s = a.editor.doc.get(id).unwrap();
    assert!(matches!(s.kind, ShapeKind::Path { .. }));
    assert_eq!(s.outline(), outline);
    run(&mut a, "variable.next", Value::Null).unwrap();
    assert_eq!(a.editor.doc.get(id).unwrap().outline(), outline, "a path no longer follows the index");
    let q = ShapeId(run(&mut a, "qr.add", json!({ "kind": "text", "text": "hi" })).unwrap()["id"].as_u64().unwrap());
    select(&mut a, &[q]);
    run(&mut a, "edit.convert_path", Value::Null).unwrap();
    assert!(matches!(a.editor.doc.get(q).unwrap().kind, ShapeKind::Path { .. }));
}

#[test]
fn fonts_and_defaults() {
    let mut a = app();
    let r = run(&mut a, "text.fonts", json!({ "query": "plex" })).unwrap();
    assert!(r["all"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().contains("Plex")));
    run(&mut a, "text.font_defaults", json!({ "font": "IBM Plex Mono", "size": 4.0, "favourite": "IBM Plex Sans" })).unwrap();
    let id = ShapeId(run(&mut a, "text.add", json!({ "text": "x" })).unwrap()["id"].as_u64().unwrap());
    let t = text_of(&a, id);
    assert_eq!((t.font.as_str(), t.height), ("IBM Plex Mono", 4.0));
    let r = run(&mut a, "text.fonts", json!({})).unwrap();
    assert_eq!(r["favourites"][0], "IBM Plex Sans");
    assert_eq!(r["recent"][0], "IBM Plex Mono");
}

#[test]
fn every_command_has_a_valid_schema() {
    for c in crate::text_cmds::commands() {
        if !c.args.is_empty() {
            assert!(serde_json::from_str::<Value>(c.args).is_ok(), "{}: {}", c.id, c.args);
        }
    }
}

#[test]
fn text_is_not_put_on_a_path_without_length() {
    let mut a = app();
    let t = add(&mut a, "Hi", json!({}));
    let before = bbox(&a, t);
    assert!(before.width() > 1.0);
    let dot = run(&mut a, "shape.add_line", json!({ "points": [[5, 5], [5, 5]], "connect": false })).unwrap()["id"].as_u64().unwrap();
    // The text used to vanish: it stayed in the project with nothing to draw or burn.
    let err = run(&mut a, "text.on_path", json!({ "text_id": t.0, "path_id": dot })).unwrap_err();
    assert!(err.contains("no length"), "{err}");
    assert!(text_of(&a, t).on_path.is_none());
    assert_eq!(bbox(&a, t), before);
}
