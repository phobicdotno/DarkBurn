//! Bug hunt 10: number fields and text helpers fed odd input.

use darkburn_ui::ui_kit::{parse_number, typed_number};
use darkburn_ui::value_step::shifted;

#[test]
fn plain_numbers_parse() {
    assert_eq!(parse_number("2,5"), Some(2.5));
    assert_eq!(parse_number(" −3 "), Some(-3.0));
    assert_eq!(parse_number("-"), None);
    assert_eq!(parse_number(","), None);
    assert_eq!(parse_number("nan"), None);
    assert_eq!(parse_number("inf"), None);
    assert_eq!(parse_number("1e999"), None);
    assert_eq!(parse_number(""), None);
}

#[test]
fn shifted_odd_text() {
    for text in ["", "-", "+", ".", ",", "é", "e\u{301}", "😀", "١٢", "−5", "1e5", "20 mm", "-0,5 mm", "1.", "99999999999999999999"] {
        let _ = shifted(text, |_| Some((1.0, 0)));
    }
}

/// A field shown with a unit takes its value typed with that unit after it, as its doc says,
/// also when the host has not installed a parser for sums and other units.
#[test]
fn typed_number_takes_its_own_unit_without_a_host_parser() {
    let ctx = egui::Context::default();
    assert_eq!(typed_number(&ctx, " mm")("20 mm"), Some(20.0));
    assert_eq!(typed_number(&ctx, " mm")("2,5mm"), Some(2.5));
    assert_eq!(typed_number(&ctx, " deg")("-45 deg"), Some(-45.0));
    assert_eq!(typed_number(&ctx, " mm")("20"), Some(20.0));
    assert_eq!(typed_number(&ctx, " mm")("mm"), None);
    assert_eq!(typed_number(&ctx, " mm")("20 in"), None);
}
