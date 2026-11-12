//! Save and open must give back the same document: every field of every shape kind, every
//! layer setting, the layer order, images, notes and the project-wide settings.

use std::sync::Arc;

use darkburn_core::kurbo::{Affine, BezPath};
use darkburn_core::layer::LAYER_COUNT;
use darkburn_core::{
    AlongDef, ArrayDef, ArrayKind, CircularDef, CodeShape, CodeSpec, CutDirection, Document, GridDef, GridShift, ImageAdjust, ImageData, ImageShape,
    PathText, Shape, ShapeKind, TextShape, project,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Changes every number and every true/false in `v`, so no field is left at its default.
fn perturb(v: &mut Value, n: &mut u32) {
    match v {
        Value::Bool(b) => *b = !*b,
        Value::Number(x) => {
            *n += 1;
            *v = if let Some(u) = x.as_u64() {
                Value::from(u + 1 + u64::from(*n % 3))
            } else if let Some(i) = x.as_i64() {
                Value::from(i - 1 - i64::from(*n % 3))
            } else {
                Value::from(x.as_f64().unwrap_or(0.0) + 0.25 + f64::from(*n % 5) * 0.125)
            };
        }
        Value::Array(a) => a.iter_mut().for_each(|x| perturb(x, n)),
        Value::Object(o) => o.iter_mut().for_each(|(_, x)| perturb(x, n)),
        Value::Null | Value::String(_) => {}
    }
}

/// `value` with every number and flag in it changed.
fn everything_changed<T: Serialize + DeserializeOwned>(value: &T) -> T {
    let mut json = serde_json::to_value(value).unwrap();
    perturb(&mut json, &mut 0);
    serde_json::from_value(json).unwrap()
}

fn path(svg: &str) -> BezPath {
    BezPath::from_svg(svg).unwrap()
}

fn dressed(mut s: Shape, n: usize) -> Shape {
    s.name = format!("shape {n} æøå \"quoted\"");
    s.transform = Affine::new([1.5, 0.25, -0.5, 2.0, 10.0 + n as f64, -7.5]);
    s.layer = (n * 5 + 1) % LAYER_COUNT;
    s.locked = n.is_multiple_of(2);
    s.priority = n as i32 - 3;
    s.power_scale = 40.0 + n as f64;
    s.tabs = vec![[1.0, 2.5], [-3.0, 0.125]];
    s.start_point = Some([0.5, -0.5]);
    s.extra_start_points = vec![[4.0, 4.0]];
    s.direction = CutDirection::ALL[1 + n % 3];
    s
}

fn every_kind(doc: &mut Document) -> Vec<Shape> {
    let key = doc.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"\x89PNG not really"[..]), px_width: 640, px_height: 480 });
    let text = everything_changed(&TextShape {
        text: "Line one\nline two {serial}".into(),
        font: "Some Font".into(),
        backup: Some(path("M0 0 L10 0 L10 5 Z")),
        on_path: Some(PathText { path: path("M0 0 C10 10 20 10 30 0"), ..Default::default() }),
        ..Default::default()
    });
    let image = ImageShape {
        image: key,
        adjust: everything_changed(&ImageAdjust::default()),
        crop: Some(path("M-5 -5 L5 -5 L5 5 L-5 5 Z")),
        source: Some("C:\\pictures\\dog.png".into()),
        backup: Some(path("M-1 -1 L1 -1 L1 1 Z")),
        ..everything_changed(&ImageShape::default())
    };
    let qr = everything_changed(&CodeShape { cache: Some(path("M0 0 L1 0 L1 1 Z")), error: Some("too long".into()), ..Default::default() });
    let barcode = everything_changed(&CodeShape { spec: CodeSpec::Barcode(Default::default()), ..Default::default() });
    let kinds = vec![
        ShapeKind::Path { path: path("M0 0 L10 0 Q15 5 10 10 C5 15 0 15 0 10 Z M2 2 L3 3") },
        ShapeKind::Rect { width: 20.5, height: 10.25, radius: -2.5 },
        ShapeKind::Ellipse { rx: 4.5, ry: 2.25 },
        ShapeKind::Polygon { sides: 7, radius: 12.5 },
        ShapeKind::Text(text),
        ShapeKind::Code(qr),
        ShapeKind::Code(barcode),
        ShapeKind::Image(image),
    ];
    kinds.into_iter().enumerate().map(|(n, k)| dressed(Shape::new(k, Affine::IDENTITY, 0), n)).collect()
}

#[test]
fn every_field_of_every_shape_and_layer_survives_save_and_open() {
    let mut doc = Document::new();
    let shapes = every_kind(&mut doc);
    for s in &shapes {
        doc.add(s.clone());
    }
    // A group of everything, a group in a group, and one virtual array of each kind.
    let inner = dressed(Shape::new(ShapeKind::Group { children: shapes[..3].to_vec() }, Affine::IDENTITY, 2), 11);
    let mut all = shapes.clone();
    all.push(inner);
    doc.add(dressed(Shape::new(ShapeKind::Group { children: all }, Affine::IDENTITY, 3), 12));
    let arrays = [
        ArrayKind::Grid(GridDef { shift: GridShift::AltRows(3.5), ..everything_changed(&GridDef::default()) }),
        ArrayKind::Grid(GridDef { shift: GridShift::Hex, ..Default::default() }),
        ArrayKind::Circular(everything_changed(&CircularDef::default())),
        ArrayKind::AlongPath(everything_changed(&AlongDef { path: path("M0 0 L100 0 L100 50"), ..Default::default() })),
    ];
    for (n, kind) in arrays.into_iter().enumerate() {
        let mut g = dressed(Shape::new(ShapeKind::Group { children: Vec::new() }, Affine::IDENTITY, 4), 20 + n);
        g.array = Some(Box::new(ArrayDef { source: vec![shapes[1].clone(), shapes[4].clone()], kind }));
        let mut next = 1000 * (n as u64 + 1);
        darkburn_core::varray::regenerate(&mut g, || {
            next += 1;
            darkburn_core::ShapeId(next)
        });
        doc.add(g);
    }
    // Every layer with every setting changed, sub-layers, links and a shuffled output order.
    for i in 0..LAYER_COUNT {
        let mut l = everything_changed(doc.layer(i));
        l.index = i;
        l.name = format!("Layer {i} ø");
        l.library_link = i.is_multiple_of(2).then(|| format!("Birch {i} mm"));
        for k in 0..i % 4 {
            l.add_sub_layer();
            l.settings[k + 1].name = format!("sub {k}");
            l.settings[k + 1].speed += 10.0 * (k + 1) as f64;
        }
        for s in &mut l.settings {
            s.sanitise();
        }
        *doc.layer_mut(i) = l;
    }
    doc.layer_order.reverse();
    doc.layer_order.swap(3, 17);
    doc.notes = "Use 3 mm birch\nsecond line ø".into();
    doc.show_notes_on_load = true;
    doc.job_settings = Some(serde_json::json!({ "version": 1, "origin": "current", "nested": { "a": [1, 2.5, null] } }));
    doc.vision_settings = Some(serde_json::json!({ "version": 2, "rotary": { "enabled": true, "diameter": 42.5 } }));
    doc.variables = everything_changed(&doc.variables);

    let bytes = project::save(&doc, "test", None).unwrap();
    let back = project::load(&bytes).unwrap().doc;
    assert_eq!(back.len(), doc.len());
    for (a, b) in back.iter().zip(doc.iter()) {
        assert_eq!(a, b, "shape {} ({})", b.id.0, b.kind_name());
    }
    for i in 0..LAYER_COUNT {
        assert_eq!(back.layer(i), doc.layer(i), "layer {i}");
    }
    assert_eq!(back, doc);
    // And once more: what was opened saves to the very same document again.
    let again = project::load(&project::save(&back, "test", None).unwrap()).unwrap().doc;
    assert_eq!(again, doc);
}
