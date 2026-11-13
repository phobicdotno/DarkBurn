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
