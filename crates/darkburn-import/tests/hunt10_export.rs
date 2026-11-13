//! Bug hunt 10: export faults found by exporting and importing again.

use darkburn_import::dxf_export::{self, DxfExportOptions, DxfVersion, LayerNaming};
use darkburn_import::dxf_import::DxfOptions;
use darkburn_import::*;
use kurbo::BezPath;

fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x0, y0));
    p.line_to((x1, y1));
    p
}

#[test]
fn dxf_items_sharing_a_layer_keep_their_own_colours() {
    // Two items on one layer, red and blue. The layer took the first colour and every
    // entity was written "by layer", so the blue line came back red.
    for version in [DxfVersion::R12, DxfVersion::R2000, DxfVersion::R2007] {
        let items = vec![
            Item::path(line(0.0, 0.0, 10.0, 0.0), Some(Rgb(255, 0, 0)), None).with_layer("Cut"),
            Item::path(line(0.0, 5.0, 10.0, 5.0), Some(Rgb(0, 0, 255)), None).with_layer("Cut"),
        ];
        let opts = DxfExportOptions { layers: LayerNaming::ByItemLayer, version, ..Default::default() };
        let out = dxf_export::export(&items, &opts);
        let doc = dxf_import::import(&out.bytes, &DxfOptions::default()).unwrap();
        let mut colours: Vec<(f64, Option<Rgb>)> = doc.items.iter().map(|i| (i.bounds().unwrap().y0, i.colour())).collect();
        colours.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(colours.len(), 2, "{version:?}");
        assert_eq!(colours[0].1, Some(Rgb(255, 0, 0)), "{version:?}");
        assert_eq!(colours[1].1, Some(Rgb(0, 0, 255)), "{version:?}");
    }
}

#[test]
fn ai_export_of_huge_or_infinite_coordinates_writes_no_inf() {
    // The page size went through f32, so 1e300 mm (or an infinite point) wrote
    // "MediaBox[0 0 inf inf]", which no PDF reader accepts.
    for far in [1e300, f64::INFINITY] {
        let items = vec![Item::path(line(0.0, 0.0, far, far), Some(Rgb(255, 0, 0)), None)];
        let ai = ai_export::export(&items, &ai_export::AiExportOptions::default());
        let s = String::from_utf8_lossy(&ai.bytes);
        assert!(!s.contains("inf") && !s.contains("NaN"), "{far}: {s}");
    }
}

fn text_item(text: &str) -> Item {
    Item::new(ItemKind::Text(TextItem {
        text: text.into(),
        font: "Arial".into(),
        style: None,
        height: 5.0,
        h_align: HAlign::Left,
        v_align: VAlign::Baseline,
        transform: kurbo::Affine::IDENTITY,
        colour: Rgb(0, 0, 0),
        outline: None,
        attributes: Default::default(),
    }))
}

#[test]
fn dxf_text_with_windows_line_breaks_writes_no_carriage_return() {
    // Only '\n' was replaced, so "a\r\nb" left a bare carriage return inside a DXF value.
    let out = dxf_export::export(&[text_item("a\r\nb\rc")], &DxfExportOptions::default());
    let doc = dxf_import::import(&out.bytes, &DxfOptions::default()).unwrap();
    let texts: Vec<String> = doc
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["a b c".to_string()]);
}

#[test]
fn dxf_spline_with_an_infinite_knot_is_read_without_nan() {
    // "1e999" reads as infinity. The knot check only asked for a rising vector, so the
    // span 0..inf was blossomed into NaN control points.
    let mut e = String::from("0\nSPLINE\n8\n0\n70\n8\n71\n3\n72\n8\n73\n4\n74\n0\n");
    for k in ["0", "0", "0", "0", "1e999", "1e999", "1e999", "1e999"] {
        e.push_str(&format!("40\n{k}\n"));
    }
    for (x, y) in [(0, 0), (10, 10), (20, 10), (30, 0)] {
        e.push_str(&format!("10\n{x}\n20\n{y}\n30\n0\n"));
    }
    let text = format!("0\nSECTION\n2\nENTITIES\n{e}0\nENDSEC\n0\nEOF\n");
    let doc = dxf_import::import(text.as_bytes(), &DxfOptions::default()).unwrap();
    let b: Vec<kurbo::Rect> = doc.items.iter().filter_map(|i| i.bounds()).collect();
    assert_eq!(b.len(), 1, "{b:?}");
    let r = b[0];
    assert!(r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite(), "{r:?}");
    assert!((r.x0 - 0.0).abs() < 1e-9 && (r.x1 - 30.0).abs() < 1e-9, "{r:?}");
}

#[test]
fn dxf_text_with_nan_size_writes_no_nan() {
    let mut t = text_item("x");
    if let ItemKind::Text(tt) = &mut t.kind {
        tt.height = f64::NAN;
        tt.transform = kurbo::Affine::new([f64::NAN, 0.0, 0.0, 1.0, f64::INFINITY, 0.0]);
    }
    let out = dxf_export::export(&[t], &DxfExportOptions::default());
    let s = String::from_utf8_lossy(&out.bytes);
    let bad: Vec<&str> = s.lines().map(str::trim).filter(|l| ["nan", "inf", "-inf"].contains(&l.to_lowercase().as_str())).collect();
    assert!(bad.is_empty(), "{bad:?}");
}
