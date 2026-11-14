//! Bug hunt 10: import faults found by reading the readers.

use darkburn_import::hpgl::{self, HpglOptions};
use darkburn_import::*;

fn path_bounds(doc: &Imported) -> Vec<kurbo::Rect> {
    doc.items.iter().filter_map(|i| i.bounds()).collect()
}

#[test]
fn hpgl_edge_rectangle_right_after_coordinates_is_read() {
    // HPGL lets an instruction follow the last number without a `;`. The `E` of `EA`
    // was taken as an exponent, so the last coordinate and the rectangle were lost.
    let doc = hpgl::import(b"IN;SP1;PU0,0;PD400,0EA800,800;PU;", &HpglOptions::default()).unwrap();
    let b = path_bounds(&doc);
    let near = |r: &kurbo::Rect, x0: f64, y0: f64, x1: f64, y1: f64| {
        (r.x0 - x0).abs() < 1e-6 && (r.y0 - y0).abs() < 1e-6 && (r.x1 - x1).abs() < 1e-6 && (r.y1 - y1).abs() < 1e-6
    };
    assert!(b.iter().any(|r| near(r, 0.0, 0.0, 10.0, 0.0)), "line 0..10 mm missing: {b:?}");
    assert!(b.iter().any(|r| near(r, 10.0, 0.0, 20.0, 20.0)), "EA rectangle missing: {b:?}");
}
