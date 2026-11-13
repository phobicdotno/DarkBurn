//! Bug hunt 10, import: PDF, DXF text and the exporters. Every file is written by the test.

use darkburn_import::dxf_import::{self, DxfOptions};
use darkburn_import::*;
use kurbo::Point;

fn dxf_of(entities: &str) -> Imported {
    let text = format!("0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n");
    dxf_import::import(text.as_bytes(), &DxfOptions::default()).unwrap()
}

fn texts(doc: &Imported) -> Vec<&TextItem> {
    doc.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// TEXT justification 72=4 is "Middle": the text is centred both ways on its second point
/// (the DXF reference gives it with vertical code 73=0). It came in centred across but sitting
/// on its baseline, so it landed half a text height too high.
#[test]
fn dxf_text_justified_middle_is_centred_both_ways() {
    let doc = dxf_of("0\nTEXT\n8\n0\n10\n1\n20\n2\n40\n4\n1\nMid\n72\n4\n73\n0\n11\n30\n21\n40\n");
    let t = texts(&doc)[0];
    assert_eq!(t.h_align, HAlign::Center);
    assert_eq!(t.v_align, VAlign::Middle, "Middle text centres vertically too");
    let o = t.transform * Point::ZERO;
    assert!((o.x - 30.0).abs() < 1e-9 && (o.y - 40.0).abs() < 1e-9, "anchored at the second point, got {o:?}");
}
