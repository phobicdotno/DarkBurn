//! Headless tests of the file formats through the app's command layer, with fixtures made here.

use std::path::{Path, PathBuf};

use darkburn_core::kurbo::{Point, Rect};
use darkburn_core::layer::Mode;
use darkburn_core::{Shape, ShapeKind};
use serde_json::{Value, json};

use super::*;
use crate::api::execute_headless;
use crate::prefs::Prefs;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let d = std::env::temp_dir().join(format!("darkburn-fileio-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.file(name);
        std::fs::write(&p, bytes).unwrap();
        p
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

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A small design on three layers, with cut settings and notes.
fn design() -> DarkBurn {
    let mut a = app();
    run(&mut a, "shape.add_rect", json!({ "x": 20, "y": 30, "width": 40, "height": 20, "layer": 1 })).unwrap();
    run(&mut a, "shape.add_ellipse", json!({ "x": 100, "y": 40, "width": 30, "height": 30, "layer": 2 })).unwrap();
    run(&mut a, "shape.add_polygon", json!({ "cx": 200, "cy": 100, "sides": 6, "radius": 15, "layer": 3 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 1, "field": "speed", "value": 321 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 1, "field": "max_power", "value": 77 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 1, "field": "name", "value": "Outline" })).unwrap();
    run(&mut a, "layer.set_mode", json!({ "layer": 2, "mode": "fill" })).unwrap();
    a.editor.doc.notes = "Cut the outline first".into();
    a.editor.doc.show_notes_on_load = true;
    a.editor.selection.clear();
    a
}

fn bbox(a: &DarkBurn) -> Rect {
    a.editor.doc.bbox().unwrap()
}

fn leaves(a: &DarkBurn) -> usize {
    a.editor.doc.iter().flat_map(|s| s.leaves()).count()
}

fn near(a: Rect, b: Rect, tol: f64) -> bool {
    (a.x0 - b.x0).abs() < tol && (a.y0 - b.y0).abs() < tol && (a.x1 - b.x1).abs() < tol && (a.y1 - b.y1).abs() < tol
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, 90, 255]);
        }
    }
    import::rgba_to_png(w, h, px).unwrap()
}

// ----- Fixtures -----

fn dxf(units: Option<i32>, lines: &[((f64, f64), (f64, f64), Option<u8>)]) -> String {
    let mut s = String::new();
    if let Some(u) = units {
        s.push_str(&format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{u}\n0\nENDSEC\n"));
    }
    s.push_str("0\nSECTION\n2\nENTITIES\n");
    for ((x1, y1), (x2, y2), colour) in lines {
        s.push_str("0\nLINE\n8\n0\n");
        if let Some(c) = colour {
            s.push_str(&format!("62\n{c}\n"));
        }
        s.push_str(&format!("10\n{x1}\n20\n{y1}\n30\n0\n11\n{x2}\n21\n{y2}\n31\n0\n"));
    }
    s.push_str("0\nENDSEC\n0\nEOF\n");
    s
}

/// A PDF with `pages` pages; page n draws a rectangle (30 + 10 n) wide.
fn pdf(pages: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: String| {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", offsets.len()).as_bytes());
    };
    obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    obj(&mut out, format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" ")));
    for i in 0..pages {
        obj(&mut out, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents {} 0 R >>", 4 + 2 * i));
        let content = format!("1 0 0 RG 20 20 {} 60 re S", 30 + 10 * i);
        obj(&mut out, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

const PLT: &str = "IN;SP1;PU0,0;PD4000,0,4000,2000,0,2000,0,0;PU;SP2;PU1000,500;PD3000,500,3000,1500;PU;";

const LBRN_V0: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.04" FormatVersion="0" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Cut"><index Value="0"/><name Value="Outline"/><speed Value="12"/><maxPower Value="55"/><minPower Value="40"/><numPasses Value="2"/><priority Value="1"/></CutSetting>
    <CutSetting type="Scan"><index Value="1"/><name Value="Engrave"/><speed Value="300"/><maxPower Value="30"/><interval Value="0.1"/><priority Value="0"/></CutSetting>
    <Shape Type="Rect" CutIndex="0" W="20" H="10" Cr="0"><XForm>1 0 0 1 50 60</XForm></Shape>
    <Shape Type="Ellipse" CutIndex="1" Rx="5" Ry="5"><XForm>1 0 0 1 100 20</XForm></Shape>
    <Notes ShowOnLoad="1" Notes="from the old file"/>
</LightBurnProject>
"#;

// ----- Round trips through the command layer -----

#[test]
fn svg_export_imports_back_at_the_same_size() {
    let dir = TempDir::new("svg");
    let mut a = design();
    let before = bbox(&a);
    let file = dir.file("out.svg");
    run(&mut a, "project.export", json!({ "path": path_arg(&file) })).unwrap();
    assert!(std::fs::read_to_string(&file).unwrap().contains("<svg"));
    let mut b = app();
    run(&mut b, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    let after = bbox(&b);
    assert!((after.width() - before.width()).abs() < 0.5 && (after.height() - before.height()).abs() < 0.5, "{before:?} vs {after:?}");
    assert_eq!(leaves(&b), 3);
}

#[test]
fn dxf_export_round_trips_with_arcs_or_segments_and_layer_names() {
    let dir = TempDir::new("dxf");
    let mut a = design();
    let before = bbox(&a);
    let arcs = dir.file("arcs.dxf");
    let lines = dir.file("lines.dxf");
    run(&mut a, "project.export", json!({ "path": path_arg(&arcs), "dxf_arcs": true, "dxf_layer_names": true })).unwrap();
    run(&mut a, "project.export", json!({ "path": path_arg(&lines), "dxf_arcs": false })).unwrap();
    let text = std::fs::read_to_string(&arcs).unwrap();
    assert!(text.contains("Outline"), "DXF layers are named after the cut layers");
    assert!(text.contains("CIRCLE"), "a circle is written as a circle when arcs are on");
    assert!(!std::fs::read_to_string(&lines).unwrap().contains("CIRCLE"), "segments only when arcs are off");
    for file in [&arcs, &lines] {
        let mut b = app();
        run(&mut b, "project.import", json!({ "paths": [path_arg(file)] })).unwrap();
        let after = bbox(&b);
        assert!((after.width() - before.width()).abs() < 0.5 && (after.height() - before.height()).abs() < 0.5, "{file:?}: {after:?}");
        assert_eq!(leaves(&b), 3);
    }
}

#[test]
fn illustrator_export_imports_back() {
    let dir = TempDir::new("ai");
    let mut a = design();
    let before = bbox(&a);
    let file = dir.file("out.ai");
    run(&mut a, "project.export", json!({ "path": path_arg(&file) })).unwrap();
    assert!(std::fs::read(&file).unwrap().starts_with(b"%PDF"));
    let mut b = app();
    run(&mut b, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    let after = bbox(&b);
    assert!((after.width() - before.width()).abs() < 0.5 && (after.height() - before.height()).abs() < 0.5, "{before:?} vs {after:?}");
}

#[test]
fn lightburn_round_trip_keeps_shapes_layers_and_notes() {
    let dir = TempDir::new("lbrn2");
    let mut a = design();
    let before = bbox(&a);
    let file = dir.file("out.lbrn2");
    run(&mut a, "project.export", json!({ "path": path_arg(&file) })).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("<LightBurnProject") && text.contains("FormatVersion=\"1\""));
    let mut b = app();
    run(&mut b, "project.open", json!({ "path": path_arg(&file) })).unwrap();
    assert!(near(bbox(&b), before, 0.01), "{:?} vs {before:?}", bbox(&b));
    assert_eq!(leaves(&b), 3);
    let l1 = b.editor.doc.layer(1);
    assert_eq!((l1.name.as_str(), l1.main().speed, l1.main().max_power), ("Outline", 321.0, 77.0));
    assert_eq!(b.editor.doc.layer(2).main().mode, Mode::Fill);
    assert_eq!(b.editor.doc.notes, "Cut the outline first");
    assert!(b.notes_open, "the notes show when the project opens");
    assert!(b.project_path.is_none(), "a LightBurn file is never saved over");
    assert!(b.editor.is_modified());
    // Every shape is on the layer it was exported from.
    let layers: Vec<usize> = b.editor.doc.iter().map(|s| s.layer).collect();
    assert_eq!(layers.len(), 3);
    for l in [1, 2, 3] {
        assert!(layers.contains(&l), "layer {l} survives: {layers:?}");
    }
}

#[test]
fn the_old_lbrn_format_opens_as_a_project_with_cut_settings_and_notes() {
    let dir = TempDir::new("lbrn");
    let file = dir.write("old.lbrn", LBRN_V0.as_bytes());
    let mut a = app();
    run(&mut a, "project.open", json!({ "path": path_arg(&file) })).unwrap();
    assert_eq!(a.editor.doc.len(), 2);
    let l0 = a.editor.doc.layer(0);
    assert_eq!((l0.name.as_str(), l0.main().speed, l0.main().max_power, l0.main().passes, l0.main().mode), ("Outline", 12.0, 55.0, 2, Mode::Line));
    let l1 = a.editor.doc.layer(1);
    assert_eq!((l1.main().mode, l1.main().interval), (Mode::Fill, 0.1));
    assert_eq!(a.editor.doc.notes, "from the old file");
    assert!(a.notes_open);
    // Priority 0 (the Scan layer) burns before priority 1.
    let order = &a.editor.doc.layer_order;
    assert!(order.iter().position(|l| *l == 1) < order.iter().position(|l| *l == 0));
    // The rectangle is 20 x 10 and, with the y flip about the 400 mm work area, centred at y 340.
    let rect = a.editor.doc.iter().find(|s| s.layer == 0).unwrap().bbox();
    assert!((rect.width() - 20.0).abs() < 1e-6 && (rect.height() - 10.0).abs() < 1e-6);
    assert!((rect.center().y - 340.0).abs() < 1e-6 && (rect.center().x - 50.0).abs() < 1e-6, "{rect:?}");
}

#[test]
fn a_lightburn_file_imports_as_a_merge_without_replacing_the_project() {
    let dir = TempDir::new("lbrnmerge");
    let file = dir.write("merge.lbrn", LBRN_V0.as_bytes());
    let mut a = app();
    run(&mut a, "shape.add_rect", json!({ "x": 0, "y": 0, "width": 10, "height": 10, "layer": 0 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 999 })).unwrap();
    run(&mut a, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert_eq!(a.editor.doc.len(), 2, "the old shape and the imported group");
    assert_eq!(a.editor.doc.layer(0).main().speed, 999.0, "a layer already in use keeps its settings");
    assert_eq!(a.editor.doc.layer(1).main().mode, Mode::Fill, "an unused layer takes the file's settings");
    assert_eq!(a.editor.history.len(), 3);
}

#[test]
fn plt_imports_pens_as_colours_on_layers() {
    let dir = TempDir::new("plt");
    let file = dir.write("cut.plt", PLT.as_bytes());
    let mut a = app();
    run(&mut a, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert_eq!(leaves(&a), 2);
    let bb = bbox(&a);
    assert!((bb.width() - 100.0).abs() < 1e-6 && (bb.height() - 50.0).abs() < 1e-6, "40 units per mm: {bb:?}");
    let layers: std::collections::BTreeSet<usize> = a.editor.doc.iter().flat_map(|s| s.leaves()).map(|s| s.layer).collect();
    assert_eq!(layers.len(), 2, "pen 1 and pen 2 land on different layers");
    a.prefs.files.hpgl_units_per_mm = 20.0;
    let mut b = app();
    b.prefs.files.hpgl_units_per_mm = 20.0;
    run(&mut b, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert!((bbox(&b).width() - 200.0).abs() < 1e-6, "the precision setting is used");
}

#[test]
fn a_multi_page_pdf_asks_for_a_page_and_imports_the_one_chosen() {
    let dir = TempDir::new("pdf");
    let file = dir.write("two.pdf", &pdf(2));
    let a = app();
    let opts = read_options(&a);
    match formats::read_file(&file, &opts).unwrap() {
        ReadOutcome::NeedsPage(n) => assert_eq!(n, 2),
        other => panic!("expected a page prompt: {other:?}"),
    }
    let mut a = app();
    let r = run(&mut a, "project.import", json!({ "paths": [path_arg(&file)], "pdf_page": 2 })).unwrap();
    assert!(!r["imported"].as_array().unwrap().is_empty());
    assert!((bbox(&a).width() - 40.0 * 25.4 / 72.0).abs() < 0.2, "page 2 is 40 points wide: {:?}", bbox(&a));
    let mut b = app();
    run(&mut b, "project.import", json!({ "paths": [path_arg(&file)], "pdf_page": "all" })).unwrap();
    assert_eq!(leaves(&b), 2, "both pages");
    // A single-page PDF needs no prompt.
    let one = dir.write("one.pdf", &pdf(1));
    assert!(matches!(formats::read_file(&one, &opts).unwrap(), ReadOutcome::Loaded(_)));
}

#[test]
fn dxf_units_can_come_from_the_file_a_fallback_or_be_forced() {
    let dir = TempDir::new("units");
    let inches = dir.write("in.dxf", dxf(Some(1), &[((0.0, 0.0), (10.0, 0.0), None)]).as_bytes());
    let none = dir.write("none.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None)]).as_bytes());
    let width = |a: &DarkBurn| bbox(a).width();
    let import = |a: &mut DarkBurn, f: &Path| {
        run(a, "project.import", json!({ "paths": [path_arg(f)] })).unwrap();
    };
    let mut a = app();
    import(&mut a, &inches);
    assert!((width(&a) - 254.0).abs() < 1e-6, "the file says inches");
    let mut b = app();
    import(&mut b, &none);
    assert!((width(&b) - 10.0).abs() < 1e-6, "automatic: millimetres");
    let mut c = app();
    c.prefs.files.dxf_units = formats::UnitMode::Fallback;
    c.prefs.files.dxf_unit = formats::LengthUnit::Inch;
    import(&mut c, &none);
    assert!((width(&c) - 254.0).abs() < 1e-6, "fallback: inches when the file has no units");
    let mut d = app();
    d.prefs.files.dxf_units = formats::UnitMode::Fallback;
    d.prefs.files.dxf_unit = formats::LengthUnit::Cm;
    import(&mut d, &inches);
    assert!((width(&d) - 254.0).abs() < 1e-6, "fallback does not override the file's units");
    let mut e = app();
    e.prefs.files.dxf_units = formats::UnitMode::Forced;
    e.prefs.files.dxf_unit = formats::LengthUnit::Cm;
    import(&mut e, &inches);
    assert!((width(&e) - 100.0).abs() < 1e-6, "forced: centimetres whatever the file says");
}

#[test]
fn dxf_auto_join_tolerance_joins_nearly_touching_lines() {
    let dir = TempDir::new("join");
    let file = dir.write("gap.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None), ((10.02, 0.0), (20.0, 0.0), None)]).as_bytes());
    let mut a = app();
    run(&mut a, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert_eq!(leaves(&a), 2);
    let mut b = app();
    b.prefs.files.auto_join = true;
    b.prefs.files.auto_join_tolerance = 0.05;
    run(&mut b, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert_eq!(leaves(&b), 1, "one path after joining");
}

#[test]
fn colours_map_to_the_closest_layer_or_to_a_layer_each() {
    let dir = TempDir::new("colours");
    let lines = [((0.0, 0.0), (10.0, 0.0), Some(1)), ((0.0, 5.0), (10.0, 5.0), Some(1)), ((0.0, 10.0), (10.0, 10.0), Some(5))];
    let file = dir.write("c.dxf", dxf(None, &lines).as_bytes());
    let layers_of = |a: &DarkBurn| -> Vec<usize> {
        let mut v: Vec<usize> = a.editor.doc.iter().flat_map(|s| s.leaves()).map(|s| s.layer).collect();
        v.sort_unstable();
        v
    };
    let mut closest = app();
    run(&mut closest, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    let mut unique = app();
    unique.prefs.files.colour_map = formats::ColourMap::Unique;
    run(&mut unique, "project.import", json!({ "paths": [path_arg(&file)] })).unwrap();
    assert_eq!(layers_of(&unique), vec![0, 0, 1], "two colours: the first appears twice on layer 0, the second on layer 1");
    let c = layers_of(&closest);
    let distinct: std::collections::BTreeSet<_> = c.iter().collect();
    assert_eq!(distinct.len(), 2, "two colours, two layers: {c:?}");
}

#[test]
fn several_files_of_every_kind_import_as_one_undo_step() {
    let dir = TempDir::new("many");
    let svg = dir.write(
        "a.svg",
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="40mm"><rect width="40" height="40" fill="none" stroke="red"/></svg>"#,
    );
    let dxf_file = dir.write("b.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None), ((10.0, 0.0), (10.0, 10.0), None)]).as_bytes());
    let pdf_file = dir.write("c.pdf", &pdf(1));
    let plt = dir.write("d.plt", PLT.as_bytes());
    let lbrn = dir.write("e.lbrn", LBRN_V0.as_bytes());
    let pic = dir.write("f.png", &png(20, 10));
    let bad = dir.write("g.dxf", b"this is not a dxf");
    let mut a = app();
    let paths: Vec<String> = [svg, dxf_file, pdf_file, plt, lbrn, pic, bad].iter().map(|p| path_arg(p)).collect();
    let r = run(&mut a, "project.import", json!({ "paths": paths })).unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 6, "six files, one group or shape each");
    assert_eq!(r["errors"].as_array().unwrap().len(), 1, "the broken one is reported");
    assert!(r["errors"][0].as_str().unwrap().contains("g.dxf"));
    assert_eq!(a.editor.history.len(), 1);
    a.editor.undo();
    assert!(a.editor.doc.is_empty());
}

#[test]
fn importer_warnings_are_kept_for_the_warnings_window() {
    let dir = TempDir::new("warn");
    // Text in HPGL is skipped with a warning.
    let plt = dir.write("w.plt", b"IN;SP1;PU0,0;PD400,0,400,400;LBhello\x03;");
    let mut a = app();
    let r = run(&mut a, "project.import", json!({ "paths": [path_arg(&plt)] })).unwrap();
    assert!(!r["warnings"].as_array().unwrap().is_empty(), "{r}");
    assert!(a.fio.warnings.is_some());
}

#[test]
fn images_survive_import_and_export_to_png_jpg_and_bmp() {
    let dir = TempDir::new("bitmaps");
    let pic = dir.write("p.png", &png(32, 16));
    let mut a = app();
    run(&mut a, "project.import", json!({ "paths": [path_arg(&pic)] })).unwrap();
    for (name, magic) in [("o.png", &b"\x89PNG"[..]), ("o.jpg", &b"\xff\xd8"[..]), ("o.bmp", &b"BM"[..])] {
        let out = dir.file(name);
        run(&mut a, "project.export", json!({ "path": path_arg(&out) })).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(magic), "{name}");
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (32, 16));
    }
    // Without a selected bitmap there is nothing to save as a picture.
    a.editor.selection.clear();
    let mut b = design();
    b.editor.selection.clear();
    let err = run(&mut b, "project.export", json!({ "path": path_arg(&dir.file("x.png")) })).unwrap_err();
    assert!(err.contains("bitmap"), "{err}");
    // A bitmap exports into vector formats too (SVG embeds it, DXF says it left it out).
    let svg = dir.file("img.svg");
    run(&mut a, "project.export", json!({ "path": path_arg(&svg) })).unwrap();
    assert!(std::fs::read_to_string(svg).unwrap().contains("data:image/png"));
}

#[test]
fn export_selection_or_everything_and_remember_the_last_choice() {
    let dir = TempDir::new("scope");
    let mut a = design();
    let first = a.editor.doc.iter().next().unwrap().id;
    run(&mut a, "selection.set", json!({ "ids": [first.0] })).unwrap();
    let one = dir.file("one.dxf");
    let all = dir.file("all.svg");
    run(&mut a, "project.export", json!({ "path": path_arg(&one), "selection_only": true })).unwrap();
    assert!(a.prefs.files.export_selection);
    assert_eq!(a.prefs.files.export_ext, "dxf");
    assert_eq!(a.prefs.files.export_dir.as_deref(), Some(path_arg(dir.0.as_path()).as_str()));
    run(&mut a, "project.export", json!({ "path": path_arg(&all) })).unwrap();
    assert!(!a.prefs.files.export_selection);
    assert_eq!(a.prefs.files.export_ext, "svg");
    let mut b = app();
    run(&mut b, "project.import", json!({ "paths": [path_arg(&one)] })).unwrap();
    assert_eq!(leaves(&b), 1, "only the selected rectangle");
    let mut c = app();
    run(&mut c, "project.import", json!({ "paths": [path_arg(&all)] })).unwrap();
    assert_eq!(leaves(&c), 3);
    // An empty selection or an empty project has nothing to export.
    a.editor.selection.clear();
    assert!(run(&mut a, "project.export", json!({ "path": path_arg(&one), "selection_only": true })).is_err());
    let mut empty = app();
    assert!(run(&mut empty, "project.export", json!({ "path": path_arg(&one) })).unwrap_err().contains("nothing"));
    assert!(run(&mut a, "project.export", json!({ "path": path_arg(&dir.file("x.unknown")) })).is_err());
}

#[test]
fn gweike_dxf_has_a_layer_per_material_and_nc_names_them() {
    let dir = TempDir::new("gweike");
    let mut a = design();
    let dxf_file = dir.file("g.dxf");
    let nc = dir.file("g.nc");
    let materials = json!({ "Outline": "Steel 3mm", "C02": "Steel 3mm", "C03": "Inox 1mm" });
    run(&mut a, "project.export", json!({ "path": path_arg(&dxf_file), "format": "gweike_dxf", "materials": materials })).unwrap();
    run(&mut a, "project.export", json!({ "path": path_arg(&nc), "format": "gweike_nc", "materials": materials })).unwrap();
    let text = std::fs::read_to_string(&dxf_file).unwrap();
    assert!(text.contains("Steel 3mm") && text.contains("Inox 1mm"), "layers are the materials");
    assert!(!text.contains("Outline"));
    let code = std::fs::read_to_string(&nc).unwrap();
    assert!(code.contains("(MATERIAL: Steel 3mm)") && code.contains("(MATERIAL: Inox 1mm)"));
    assert!(code.contains("G21") && code.contains("M07"));
    assert!(!code.contains("S") || !code.contains("laser"), "no power or laser commands in the file");
    assert_eq!(formats::export_layer_names(&a.editor.doc, None), vec!["Outline", "C02", "C03"]);
    // The .chf craft file is not written (MCC-02 waits for the card research).
    assert!(run(&mut a, "project.export", json!({ "path": path_arg(&dir.file("g.chf")) })).is_err());
}

#[test]
fn printing_makes_a_pdf_in_black_or_in_colour() {
    let dir = TempDir::new("print");
    let mut a = design();
    let black = dir.file("black.pdf");
    let colour = dir.file("colour.pdf");
    run(&mut a, "project.print_pdf", json!({ "path": path_arg(&black), "black_only": true })).unwrap();
    run(&mut a, "project.print_pdf", json!({ "path": path_arg(&colour), "black_only": false })).unwrap();
    let (b, c) = (std::fs::read(&black).unwrap(), std::fs::read(&colour).unwrap());
    assert!(b.starts_with(b"%PDF") && c.starts_with(b"%PDF"));
    assert_ne!(b, c, "colours differ");
    let mut back = app();
    run(&mut back, "project.import", json!({ "paths": [path_arg(&black)] })).unwrap();
    assert_eq!(leaves(&back), 3);
    for s in back.editor.doc.iter().flat_map(|s| s.leaves()) {
        assert_eq!(s.layer, 0, "all black lands on the black layer");
    }
    let mut empty = app();
    assert!(run(&mut empty, "project.print_pdf", json!({ "path": path_arg(&black) })).is_err());
}

#[test]
fn preferences_export_import_and_backups_work_through_commands() {
    let dir = TempDir::new("prefs");
    let mut a = app();
    a.settings_dir = Some(dir.file("settings"));
    a.prefs.theme = "nord".into();
    a.prefs.files.dxf_arcs = false;
    let file = dir.file("p.json");
    run(&mut a, "project.prefs_export", json!({ "path": path_arg(&file) })).unwrap();
    let mut b = app();
    b.settings_dir = Some(dir.file("settings-b"));
    run(&mut b, "project.prefs_import", json!({ "path": path_arg(&file) })).unwrap();
    assert_eq!(b.prefs.theme, "nord");
    assert!(!b.prefs.files.dxf_arcs);
    assert_eq!(prefs_files::list_backups(&dir.file("settings-b")).len(), 1);
    std::fs::write(&file, "garbage").unwrap();
    assert!(run(&mut b, "project.prefs_import", json!({ "path": path_arg(&file) })).is_err());
    assert_eq!(b.prefs.theme, "nord", "a bad file changes nothing");
}

// ----- The worker thread -----

fn wait_for_job(a: &mut DarkBurn) {
    for _ in 0..500 {
        poll_job(a);
        if a.fio.job.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the import never finished");
}

#[test]
fn imports_run_on_a_worker_thread_and_land_as_one_undo_step() {
    let dir = TempDir::new("job");
    let f1 = dir.write("a.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None), ((10.0, 0.0), (10.0, 10.0), None)]).as_bytes());
    let f2 = dir.write("b.plt", PLT.as_bytes());
    let mut a = app();
    start_import(&mut a, vec![f1, f2], None);
    assert!(a.fio.job.is_some(), "the window gets control back at once");
    wait_for_job(&mut a);
    assert_eq!(a.editor.doc.len(), 2);
    assert_eq!(a.editor.history.len(), 1);
}

#[test]
fn a_cancelled_import_adds_nothing() {
    let dir = TempDir::new("cancel");
    let f1 = dir.write("a.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None)]).as_bytes());
    let mut a = app();
    start_import(&mut a, vec![f1.clone()], None);
    a.fio.job.as_ref().unwrap().cancel.store(true, Ordering::Relaxed);
    wait_for_job(&mut a);
    assert!(a.editor.doc.is_empty());
    // And the next import works.
    start_import(&mut a, vec![f1], None);
    wait_for_job(&mut a);
    assert_eq!(a.editor.doc.len(), 1);
}

#[test]
fn opening_in_the_background_replaces_the_project_with_a_lightburn_file() {
    let dir = TempDir::new("jobopen");
    let f = dir.write("a.lbrn", LBRN_V0.as_bytes());
    let mut a = design();
    start_open(&mut a, f);
    wait_for_job(&mut a);
    assert_eq!(a.editor.doc.len(), 2);
    assert_eq!(a.editor.doc.notes, "from the old file");
}

#[test]
fn a_pdf_with_pages_waits_for_the_page_choice_in_the_background() {
    let dir = TempDir::new("jobpdf");
    let f = dir.write("a.pdf", &pdf(3));
    let mut a = app();
    start_import(&mut a, vec![f], None);
    wait_for_job(&mut a);
    let pick = a.fio.page_pick.as_ref().expect("the page picker opens");
    assert_eq!(pick.count, 3);
    assert!(a.editor.doc.is_empty());
}

#[test]
fn dropping_files_imports_them_and_hands_device_profiles_to_the_hook() {
    let dir = TempDir::new("drop");
    let f1 = dir.write("a.dxf", dxf(None, &[((0.0, 0.0), (10.0, 0.0), None)]).as_bytes());
    let dev = dir.write("laser.dbdev", b"{}");
    let mut a = app();
    crate::files::drop_files(&mut a, &egui::Context::default(), vec![f1, dev.clone()], Some(Point::new(50.0, 50.0)));
    wait_for_job(&mut a);
    assert_eq!(a.editor.doc.len(), 1);
    assert_eq!(a.fio.device_profiles, vec![dev]);
    let c = bbox(&a).center();
    assert!((c.x - 50.0).abs() < 1e-6 && (c.y - 50.0).abs() < 1e-6, "centred on the drop point");
    // A dropped LightBurn project opens as the project.
    let lb = dir.write("p.lbrn", LBRN_V0.as_bytes());
    crate::files::drop_files(&mut a, &egui::Context::default(), vec![lb], None);
    assert!(a.files_ask_or_open_pending());
}

impl DarkBurn {
    /// A dropped project either opened (unmodified project) or is waiting behind "Save changes?".
    fn files_ask_or_open_pending(&self) -> bool {
        self.editor.doc.notes == "from the old file" || self.editor.is_modified()
    }
}

// ----- Robustness -----

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn samples(dir: &TempDir) -> Vec<(PathBuf, Vec<u8>)> {
    let mut a = design();
    let pic = dir.write("s.png", &png(8, 8));
    run(&mut a, "project.import", json!({ "paths": [path_arg(&pic)] })).unwrap();
    let mut out = Vec::new();
    for name in ["s.svg", "s.dxf", "s.ai", "s.lbrn2"] {
        let f = dir.file(name);
        run(&mut a, "project.export", json!({ "path": path_arg(&f) })).unwrap();
        out.push((f.clone(), std::fs::read(&f).unwrap()));
    }
    out.push((dir.file("s.pdf"), pdf(2)));
    out.push((dir.file("s.plt"), PLT.as_bytes().to_vec()));
    out.push((dir.file("s.lbrn"), LBRN_V0.as_bytes().to_vec()));
    out.push((dir.file("s.png"), png(8, 8)));
    out
}

#[test]
fn damaged_files_of_every_kind_never_crash_the_reader() {
    let dir = TempDir::new("fuzz");
    let opts = read_options(&app());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut cases = 0;
    for (path, bytes) in samples(&dir) {
        // The undamaged file reads.
        assert!(formats::read_bytes(&path, &bytes, &opts).is_ok(), "{path:?} reads");
        for round in 0..120 {
            let mut b = bytes.clone();
            match round % 5 {
                0 => b.truncate((rng.next() as usize) % (b.len() + 1)),
                1 => {
                    for _ in 0..1 + rng.next() % 8 {
                        let i = (rng.next() as usize) % b.len().max(1);
                        if let Some(x) = b.get_mut(i) {
                            *x = rng.next() as u8;
                        }
                    }
                }
                2 => {
                    let i = (rng.next() as usize) % (b.len() + 1);
                    let junk: Vec<u8> = (0..(rng.next() % 40)).map(|_| rng.next() as u8).collect();
                    b.splice(i..i, junk);
                }
                3 => {
                    let i = (rng.next() as usize) % (b.len() + 1);
                    let j = (i + (rng.next() as usize) % 64).min(b.len());
                    b.drain(i..j);
                }
                _ => b = (0..(rng.next() % 600)).map(|_| rng.next() as u8).collect(),
            }
            let mut o = opts.clone();
            o.pdf_page = Some(if round % 2 == 0 { PageChoice::Page(1) } else { PageChoice::All });
            // `read_bytes` turns a panic inside a reader into this error, so "it returned" alone
            // proves nothing: a reader that panics on a damaged file is a bug too.
            if let Err(e) = formats::read_bytes(&path, &b, &o) {
                assert!(!e.contains("internal error"), "{path:?} round {round}: a reader panicked (its message is above)");
            }
            cases += 1;
        }
    }
    assert!(cases > 900);
}

#[test]
fn opening_broken_files_reports_an_error_and_leaves_the_project_alone() {
    let dir = TempDir::new("broken");
    let mut a = design();
    let before = a.editor.doc.clone();
    for (name, bytes) in [
        ("x.dxf", &b"0\nSECTION\n2\nENTITIES\n0\nLINE\n10\nabc"[..]),
        ("x.pdf", &b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj"[..]),
        ("x.ai", &b"%!PS-Adobe-3.0\n"[..]),
        ("x.plt", &b"\x00\x01\x02"[..]),
        ("x.lbrn2", &b"<LightBurnProject><Shape Type=\"Path\""[..]),
        ("x.lbrn", &b""[..]),
        ("x.svg", &b"<svg"[..]),
        ("x.png", &b"\x89PNG\r\n\x1a\nnope"[..]),
        ("x.sc", &b"signcut"[..]),
        ("x.unknown", &b"whatever"[..]),
    ] {
        let f = dir.write(name, bytes);
        assert!(run(&mut a, "project.open", json!({ "path": path_arg(&f) })).is_err(), "{name} is refused");
        assert!(run(&mut a, "project.import", json!({ "paths": [path_arg(&f)] })).is_err(), "{name} is refused on import");
        assert_eq!(a.editor.doc, before, "{name} left the project alone");
    }
}

#[test]
fn absurd_coordinates_and_deep_nesting_do_not_hang_or_crash() {
    let dir = TempDir::new("absurd");
    let big = dir.write("big.dxf", dxf(None, &[((0.0, 0.0), (1e300, 1e300), None), ((f64::MAX, 0.0), (1.0, 1.0), None)]).as_bytes());
    let mut a = app();
    let _ = run(&mut a, "project.import", json!({ "paths": [path_arg(&big)] }));
    let nest = format!(
        "{}{}{}",
        "<Shape Type=\"Group\"><XForm>1 0 0 1 0 0</XForm><Children>".repeat(300),
        "<Shape Type=\"Rect\" W=\"1\" H=\"1\"/>",
        "</Children></Shape>".repeat(300)
    );
    let lb = dir.write("deep.lbrn2", format!("<LightBurnProject FormatVersion=\"1\">{nest}</LightBurnProject>").as_bytes());
    let _ = run(&mut a, "project.open", json!({ "path": path_arg(&lb) }));
}

// ----- Crash restore, recent projects and windows -----

#[test]
fn an_imported_project_survives_a_crash_through_the_auto_save() {
    let dir = TempDir::new("crash");
    let settings = dir.file("settings");
    let lb = dir.write("p.lbrn", LBRN_V0.as_bytes());
    let pic = dir.write("p.png", &png(8, 8));
    let mut a = app();
    a.settings_dir = Some(settings.clone());
    a.prefs.autosave_minutes = 1.0;
    run(&mut a, "project.open", json!({ "path": path_arg(&lb) })).unwrap();
    run(&mut a, "project.import", json!({ "paths": [path_arg(&pic)] })).unwrap();
    assert_eq!(crate::files::begin_session(&settings), None);
    crate::files::write_autosave(&settings, &a.editor.doc, None).unwrap();
    // The copy is killed; the next start finds the marker and the auto-save.
    let found = crate::files::begin_session(&settings).expect("the crash is noticed");
    let mut b = app();
    crate::files::restore_autosave(&mut b, &found).unwrap();
    assert_eq!(b.editor.doc, a.editor.doc);
    assert!(b.editor.is_modified());
    assert_eq!(b.editor.doc.layer(0).main().speed, 12.0);
    // A damaged auto-save is an error, not a crash.
    std::fs::write(settings.join(crate::files::AUTOSAVE_FILE), b"PK garbage").unwrap();
    assert!(crate::files::restore_autosave(&mut b, &settings.join(crate::files::AUTOSAVE_FILE)).is_err());
}

#[test]
fn a_new_window_with_one_copy_only_starts_a_new_project_here() {
    let mut a = design();
    a.editor.mark_saved();
    a.prefs.files.single_instance = true;
    a.run(&egui::Context::default(), "file.new_window");
    assert!(a.editor.doc.is_empty());
}

#[test]
fn the_new_commands_are_registered_and_marked_as_working() {
    for id in [
        "file.export",
        "file.save_lbrn2",
        "file.export_gweike_dxf",
        "file.export_gweike_nc",
        "file.print",
        "file.print_colour",
        "file.prefs_export",
        "file.prefs_import",
        "file.prefs_backup",
        "file.import_options",
        "file.new_window",
        "project.export",
        "project.print_pdf",
        "project.prefs_export",
        "project.prefs_import",
    ] {
        let info = crate::commands::describe(id).unwrap_or_else(|| panic!("{id} is registered"));
        assert!(info.implemented, "{id}");
        assert!(!info.hardware, "{id}");
    }
    assert!(crate::commands::describe("file.save_lbrn2").unwrap().title.contains("LightBurn"), "the menu item says what it writes");
    // Menu commands only open windows; running them without a window opens nothing blocking.
    let mut a = design();
    for id in ["file.export", "file.print", "file.import_options", "file.prefs_backup", "file.export_gweike_nc"] {
        a.run(&egui::Context::default(), id);
    }
    assert!(a.fio.pick.is_none(), "no file picker is started outside the frame loop");
}

#[test]
fn recent_projects_still_track_saves_and_ignore_other_formats() {
    let dir = TempDir::new("recent");
    let lb = dir.write("p.lbrn", LBRN_V0.as_bytes());
    let mut a = app();
    run(&mut a, "project.open", json!({ "path": path_arg(&lb) })).unwrap();
    assert!(a.prefs.recent.is_empty(), "a LightBurn file is not a recent DarkBurn project");
    run(&mut a, "project.save_as", json!({ "path": path_arg(&dir.file("mine.dbrn")) })).unwrap();
    assert_eq!(a.prefs.recent.len(), 1);
    let _ = ShapeKind::Group { children: vec![] };
    let _ = Shape::default();
}

#[test]
fn text_files_with_a_byte_order_mark_still_open() {
    let dir = TempDir::new("bom");
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend(LBRN_V0.as_bytes());
    let f = dir.write("bom.lbrn", &bytes);
    let mut a = app();
    run(&mut a, "project.open", json!({ "path": path_arg(&f) })).unwrap();
    assert_eq!(a.editor.doc.len(), 2);
    let d = dir.write("bom.dxf", &[b"\xEF\xBB\xBF".to_vec(), dxf(None, &[((0.0, 0.0), (10.0, 0.0), None)]).into_bytes()].concat());
    let mut b = app();
    run(&mut b, "project.import", json!({ "paths": [path_arg(&d)] })).unwrap();
    assert_eq!(b.editor.doc.len(), 1);
}
