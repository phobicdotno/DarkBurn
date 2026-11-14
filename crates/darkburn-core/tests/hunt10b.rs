//! Bug hunt 10 (project files): damaged and odd project files and clipboard texts open without a
//! crash or a hang, and what opens keeps the rules the editor keeps.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use darkburn_core::kurbo::{Affine, BezPath, Rect};
use darkburn_core::{Clipboard, Document, ImageData, ImageShape, LAYER_COUNT, MAX_SUB_LAYERS, Shape, ShapeKind, TextShape, project};

fn sample() -> Document {
    let mut d = Document::new();
    d.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 10.0), 1.5, 3));
    d.add(Shape::ellipse(Rect::new(0.0, 0.0, 20.0, 10.0), 4));
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.curve_to((1.0, 2.0), (3.0, 4.0), (5.0, 0.5));
    d.add(Shape::path(p, 0));
    let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"not an image at all"[..]), px_width: 4, px_height: 2 });
    d.add(Shape::new(ShapeKind::Image(ImageShape { image: key, width: 40.0, height: 20.0, ..Default::default() }), Affine::rotate(0.3), 6));
    d.add(Shape::new(ShapeKind::Text(TextShape { text: "Hi".into(), ..Default::default() }), Affine::IDENTITY, 7));
    let g = vec![Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 1), Shape::rect(Rect::new(2.0, 0.0, 3.0, 1.0), 0.0, 2)];
    d.add(Shape::new(ShapeKind::Group { children: g }, Affine::translate((9.0, 9.0)), 1));
    d.layer_mut(3).add_sub_layer();
    d
}

fn json_of(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    zip.by_name("project.json").unwrap().read_to_string(&mut text).unwrap();
    text
}

fn zipped(json: &str) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("project.json", zip::write::SimpleFileOptions::default()).unwrap();
    w.write_all(json.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

fn ids_of(s: &Shape, out: &mut Vec<u64>) {
    out.push(s.id.0);
    if let ShapeKind::Group { children } = &s.kind {
        children.iter().for_each(|c| ids_of(c, out));
    }
}

/// The rules an opened document keeps.
fn check(doc: &Document, what: &str) {
    assert_eq!(doc.layers.len(), LAYER_COUNT, "{what}");
    let mut order = doc.layer_order.clone();
    order.sort_unstable();
    assert_eq!(order, (0..LAYER_COUNT).collect::<Vec<_>>(), "{what}: layer order");
    for (i, l) in doc.layers.iter().enumerate() {
        assert_eq!(l.index, i, "{what}: layer index");
        assert!(!l.settings.is_empty() && l.settings.len() <= MAX_SUB_LAYERS, "{what}: sub-layers");
    }
    let mut ids = Vec::new();
    doc.iter().for_each(|s| ids_of(s, &mut ids));
    let set: HashSet<u64> = ids.iter().copied().collect();
    assert_eq!(set.len(), ids.len(), "{what}: ids used twice");
    assert!(ids.iter().all(|id| *id != 0 && *id < doc.next_id()), "{what}: ids {ids:?} next {}", doc.next_id());
    for s in doc.iter().flat_map(|s| s.leaves()) {
        assert!(s.layer < LAYER_COUNT, "{what}: layer {}", s.layer);
    }
}

/// Every number in `text` replaced, one at a time, by each of `odd`.
fn each_number_replaced(text: &str, odd: &[&str], mut f: impl FnMut(String, &str)) {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let starts =
            (b[i].is_ascii_digit() || b[i] == b'-') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'.' || b[i - 1] == b'"'));
        if starts {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_digit() || matches!(b[j], b'.' | b'e' | b'E' | b'-' | b'+')) {
                j += 1;
            }
            for o in odd {
                f(format!("{}{}{}", &text[..i], o, &text[j..]), &format!("{} at {i} -> {o}", &text[i..j]));
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

const ODD: [&str; 7] = ["NaN", "Infinity", "-0", "1e308", "-1e308", "\"x\"", "1e999"];

#[test]
fn a_project_cut_off_at_every_byte_opens_or_is_refused() {
    let bytes = project::save(&sample(), "t", Some(b"thumb")).unwrap();
    for n in 0..bytes.len() {
        if let Ok(l) = project::load(&bytes[..n]) {
            check(&l.doc, &format!("cut at {n}"));
        }
    }
    let text = json_of(&bytes);
    for n in (0..text.len()).filter(|n| text.is_char_boundary(*n)) {
        if let Ok(l) = project::load(&zipped(&text[..n])) {
            check(&l.doc, &format!("json cut at {n}"));
        }
    }
}

#[test]
fn every_number_in_a_project_made_odd_opens_or_is_refused() {
    let text = json_of(&project::save(&sample(), "t", None).unwrap());
    each_number_replaced(&text, &ODD, |t, what| {
        if let Ok(l) = project::load(&zipped(&t)) {
            check(&l.doc, what);
            let again = project::save(&l.doc, "t", None).unwrap_or_else(|e| panic!("{what}: saves again: {e}"));
            check(&project::load(&again).unwrap_or_else(|e| panic!("{what}: opens again: {e}")).doc, what);
        }
    });
}

#[test]
fn every_number_in_clipboard_text_made_odd_pastes_or_is_refused() {
    let d = sample();
    let mut c = Clipboard::default();
    let ids: Vec<_> = d.iter().map(|s| s.id).collect();
    c.copy(&d, &ids);
    let text = c.to_text();
    each_number_replaced(&text, &ODD, |t, what| {
        if let Some(mut c) = Clipboard::from_text(&t) {
            let mut into = sample();
            c.paste(&mut into, darkburn_core::PasteAt::Offset);
            check(&into, what);
        }
    });
}

#[test]
fn arrays_emptied_and_doubled_open() {
    let text = json_of(&project::save(&sample(), "t", None).unwrap());
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    fn walk(v: &serde_json::Value, path: &mut Vec<String>, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Array(a) => {
                out.push(path.join("/"));
                for (i, x) in a.iter().enumerate() {
                    path.push(i.to_string());
                    walk(x, path, out);
                    path.pop();
                }
            }
            serde_json::Value::Object(o) => {
                for (k, x) in o {
                    path.push(k.clone());
                    walk(x, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut paths = Vec::new();
    walk(&v, &mut Vec::new(), &mut paths);
    for p in paths {
        for double in [false, true] {
            let mut w = v.clone();
            let target = w.pointer_mut(&format!("/{p}")).unwrap().as_array_mut().unwrap();
            if double {
                let copy = target.clone();
                target.extend(copy);
            } else {
                target.clear();
            }
            if let Ok(l) = project::load(&zipped(&w.to_string())) {
                check(&l.doc, &format!("{p} doubled {double}"));
                project::load(&project::save(&l.doc, "t", None).unwrap()).unwrap();
            }
        }
    }
}

fn nested(depth: usize) -> Shape {
    let mut s = Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 1);
    for _ in 0..depth {
        s = Shape::new(ShapeKind::Group { children: vec![s] }, Affine::IDENTITY, 1);
    }
    s
}

fn depth_of(s: &Shape) -> usize {
    match &s.kind {
        ShapeKind::Group { children } => 1 + children.iter().map(depth_of).max().unwrap_or(0),
        _ => 0,
    }
}

/// Groups inside groups 10,000 deep, in a file or on the clipboard: refused or opened, quickly,
/// and never by running out of stack.
#[test]
fn groups_nested_ten_thousand_deep_are_refused_quickly() {
    let n = 10_000;
    let shape = r#"{"id":1,"kind":{"type":"rect","width":1.0,"height":1.0,"radius":0.0}}"#;
    let mut deep = String::from(shape);
    for i in 0..n {
        deep = format!(r#"{{"id":{},"kind":{{"type":"group","children":[{deep}]}}}}"#, i + 2);
    }
    let file = format!(r#"{{"format":"darkburn-project","version":3,"shapes":[{deep}]}}"#);
    let clip = format!(r#"{}{{"shapes":[{deep}],"images":[]}}"#, darkburn_core::clipboard::CLIPBOARD_MARK);
    let start = Instant::now();
    let h = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let _ = project::load(&zipped(&file));
            let _ = Clipboard::from_text(&clip);
        })
        .unwrap();
    while !h.is_finished() {
        assert!(start.elapsed() < Duration::from_secs(20), "still opening after 20 s");
        std::thread::sleep(Duration::from_millis(20));
    }
    h.join().expect("no crash");
}

/// A group of groups 60 deep (grouping a few times over) saves and opens as it was: the save
/// must not quietly leave it out because it reads back too deep.
#[test]
fn a_deeply_grouped_shape_survives_save_and_open() {
    // On a small stack, as deep as a file may nest (about 170 groups) still opens.
    std::thread::Builder::new().stack_size(2 << 20).spawn(deeply_grouped).unwrap().join().unwrap();
}

fn deeply_grouped() {
    for depth in [10, 40, 60, 100, 165] {
        let mut d = Document::new();
        let id = d.add(nested(depth));
        let bytes = project::save(&d, "t", None).unwrap();
        let back = project::load(&bytes).unwrap().doc;
        assert_eq!(back.len(), 1, "depth {depth}: the group is gone after save and open");
        assert_eq!(depth_of(back.get(id).unwrap()), depth);
        let mut c = Clipboard::default();
        c.copy(&d, &[id]);
        let pasted = Clipboard::from_text(&c.to_text());
        assert!(pasted.is_some_and(|c| c.shapes.len() == 1), "depth {depth}: the copied group does not paste");
    }
}

/// Ids used twice, missing, or pointing nowhere.
#[test]
fn odd_ids_open_with_ids_that_keep_the_rules() {
    for json in [
        r#"{"format":"darkburn-project","version":3,"next_id":2,"shapes":[{"id":5,"kind":{"type":"rect","width":1.0,"height":1.0,"radius":0.0}}]}"#,
        r#"{"format":"darkburn-project","version":3,"next_id":0,"shapes":[{"id":1,"kind":{"type":"rect","width":1.0,"height":1.0,"radius":0.0}}]}"#,
        r#"{"format":"darkburn-project","version":3,"next_id":9007199254740993,"shapes":[{"id":9007199254740992,"kind":{"type":"rect","width":1.0,"height":1.0,"radius":0.0}}]}"#,
        r#"{"format":"darkburn-project","version":3,"next_id":3,"shapes":[{"id":2,"kind":{"type":"group","children":[{"id":2,"kind":{"type":"rect","width":1.0,"height":1.0,"radius":0.0}}]}}]}"#,
        r#"{"format":"darkburn-project","version":3,"layers":[{"index":3,"settings":[]},{"index":99}],"layer_order":[3,3,99,0],"shapes":[]}"#,
    ] {
        let mut doc = project::load(&zipped(json)).unwrap_or_else(|e| panic!("{json}: {e}")).doc;
        check(&doc, json);
        let new = doc.add(Shape::rect(Rect::new(0.0, 0.0, 1.0, 1.0), 0.0, 0));
        check(&doc, json);
        assert!(doc.get(new).is_some());
    }
}
