//! Bug hunt 10, import/export part b: MTEXT `\X` line breaks and multi-line SVG text.

use darkburn_import::svg_export::{self, SvgExportOptions};
use darkburn_import::*;

fn text_item(text: &str, height: f64) -> Item {
    Item::new(ItemKind::Text(TextItem {
        text: text.into(),
        font: "Arial".into(),
        style: None,
        height,
        h_align: HAlign::Left,
        v_align: VAlign::Baseline,
        transform: kurbo::Affine::IDENTITY,
        colour: Rgb(0, 0, 0),
        outline: None,
        attributes: Default::default(),
    }))
}

#[test]
fn mtext_x_line_break_keeps_the_text_after_it() {
    // `\X` (the break between dimension text above and below the line) takes no argument,
    // but it was read up to the next ';', so everything after it was lost.
    assert_eq!(dxf_import::strip_mtext("Top\\XBottom"), "Top\nBottom");
    assert_eq!(dxf_import::strip_mtext("A\\XB;C"), "A\nB;C");
}

#[test]
fn svg_multi_line_text_writes_one_tspan_per_line() {
    // The whole text went into one <text> with "&#10;" in it, which viewers show on one line.
    let svg = svg_export::export(&[text_item("Top\nBottom", 5.0)], &SvgExportOptions::default());
    assert!(!svg.contains("&#10;"), "{svg}");
    assert_eq!(svg.matches("<tspan").count(), 2, "{svg}");
    // First line stays at the text origin; the second sits 1.2 x the height below it
    // (text space is Y down after the flip, so below is +y).
    assert!(svg.contains("<tspan x=\"0\" y=\"0\">Top</tspan>"), "{svg}");
    assert!(svg.contains("<tspan x=\"0\" y=\"6\">Bottom</tspan>"), "{svg}");
}

#[test]
fn svg_single_line_text_is_unchanged() {
    let svg = svg_export::export(&[text_item("Hello & bye", 5.0)], &SvgExportOptions::default());
    assert!(!svg.contains("<tspan"), "{svg}");
    assert!(svg.contains(">Hello &amp; bye</text>"), "{svg}");
}
