//! Bug hunt pass 5, files: importers and exporters.

use darkburn_import::dxf_import::{self, DxfOptions};
use darkburn_import::*;
use kurbo::{Point, Shape};

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

/// A LINE's end points are world coordinates. Its extrusion direction only says which way a
/// thickness goes, so a line whose extrusion points down must stay where the file puts it
/// (arcs and circles are different: theirs are in the object's own coordinate system).
#[test]
fn dxf_line_with_downward_extrusion_is_not_mirrored() {
    let doc = dxf_of("0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n5\n210\n0\n220\n0\n230\n-1\n");
    let ItemKind::Path(p) = &doc.items[0].kind else { panic!("a path") };
    let b = p.path.bounding_box();
    assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 10.0).abs() < 1e-9, "the line runs from x=0 to x=10, got {b:?}");
    assert!((b.y0 - 0.0).abs() < 1e-9 && (b.y1 - 5.0).abs() < 1e-9, "{b:?}");
}

/// Aligned and Fit text runs from its first point to its second. It is read as left aligned,
/// so it must be anchored at the first point, not at the far end.
#[test]
fn dxf_aligned_and_fit_text_start_at_their_first_point() {
    for code in [3, 5] {
        let doc = dxf_of(&format!("0\nTEXT\n8\n0\n10\n5\n20\n7\n40\n3\n1\nFit me\n72\n{code}\n11\n55\n21\n7\n"));
        let t = texts(&doc)[0];
        assert_eq!(t.h_align, HAlign::Left);
        let o = t.transform * Point::ZERO;
        assert!((o.x - 5.0).abs() < 1e-9 && (o.y - 7.0).abs() < 1e-9, "justification {code}: anchored at {o:?}");
    }
}
