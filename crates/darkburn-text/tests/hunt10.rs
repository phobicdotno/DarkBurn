//! Bug hunt 10: a very long Code 128 barcode, and text on a path at a place that is not a number.

mod common;
use common::*;
use darkburn_text::barcode::code128_values;
use darkburn_text::*;
use kurbo::{BezPath, Circle, Shape};

/// The check symbol is the weighted sum of all symbols modulo 103, however long the data is.
#[test]
fn a_very_long_code128_still_has_the_right_check_symbol() {
    for data in ["~".repeat(12_000), "z9".repeat(8_000), "A".to_string()] {
        let v = code128_values(&data).unwrap();
        let n = v.len();
        assert_eq!(v[n - 1], 106, "stop symbol");
        let sum: u64 = u64::from(v[0]) + v[1..n - 2].iter().enumerate().map(|(k, c)| u64::from(*c) * (k as u64 + 1)).sum::<u64>();
        assert_eq!(u64::from(v[n - 2]), sum % 103, "{} characters", data.len());
    }
}

fn layout(text: &str) -> TextLayout {
    layout_text(&TextSpec { text: text.into(), family: MONO.into(), size: 10.0, ..Default::default() }, &lib()).unwrap()
}

fn line(len: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((len, 0.0));
    p
}

/// A start or a distance from the path that is not a number has no place on the path: there is
/// no text then, and nothing with coordinates that are not numbers.
#[test]
fn text_on_a_path_at_a_place_that_is_not_a_number_gives_no_text() {
    let lay = layout("HIJ");
    let circle = Circle::new((0.0, 0.0), 30.0).to_path(0.01);
    for path in [line(200.0), circle] {
        for distort in [false, true] {
            for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                for center_on_path in [false, true] {
                    let o = PathTextOptions { start_offset: bad, center_on_path, distort, ..Default::default() };
                    assert!(text_on_path(&lay, &path, &o).is_empty(), "start {bad}, distort {distort}");
                    let o = PathTextOptions { offset: bad, center_on_path, distort, ..Default::default() };
                    assert!(text_on_path(&lay, &path, &o).is_empty(), "offset {bad}, distort {distort}");
                }
            }
            // An ordinary place still gives the text.
            let o = PathTextOptions { start_offset: 25.0, distort, ..Default::default() };
            assert_eq!(text_on_path(&lay, &path, &o).len(), lay.glyphs.len());
        }
    }
}

/// Asking the sampler itself for such a place does not crash it.
#[test]
fn a_path_sampler_asked_for_a_place_that_is_not_a_number_does_not_crash() {
    let circle = Circle::new((0.0, 0.0), 30.0).to_path(0.01);
    for path in [line(200.0), circle] {
        let s = PathSampler::new(&path, 0.01).unwrap();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let _ = s.at(bad);
        }
        let (p, _) = s.at(10.0);
        assert!(p.x.is_finite() && p.y.is_finite());
    }
}

/// Welding hands the outlines to the overlay library, which cannot take a point that is not a
/// number or absurdly far out. Such an outline has no area that could be worked out: it is left
/// out, and the rest is welded as before. A code with an absurd module size or bar height (a
/// project file can hold any number) gives no outline instead of a crash.
#[test]
fn welding_an_outline_absurdly_far_out_leaves_it_out_and_does_not_crash() {
    use darkburn_text::geomutil::weld;
    use kurbo::Rect;
    let square = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1);
    for far in [1e300, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
        let mut bad = BezPath::new();
        bad.move_to((0.0, 0.0));
        bad.line_to((far, 1.0));
        bad.line_to((0.0, 1.0));
        bad.close_path();
        let out = weld(&[bad, square.clone()], 0.01);
        assert_eq!(out.len(), 1, "{far}");
        let b = out[0].bounding_box();
        assert!((b.width() - 10.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9, "{far}: {b:?}");
    }
    for module_mm in [1e300, f64::MAX] {
        for kind in [BarcodeKind::Code128, BarcodeKind::DataMatrix] {
            let _ = BarcodeSpec { kind, data: "1234".into(), module_mm, ..Default::default() }.generate();
        }
        let _ = QrSpec { content: QrContent::Text("hi".into()), module_mm, ..Default::default() }.generate();
        let _ = BarcodeSpec { data: "1234".into(), height_mm: module_mm, ..Default::default() }.generate();
    }
    // An ordinary code is still welded into bars.
    let r = BarcodeSpec { data: "1234".into(), ..Default::default() }.generate().unwrap();
    assert!(!r.paths.is_empty());
}
