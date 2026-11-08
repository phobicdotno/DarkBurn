//! Performance check: union and offset 2,000 overlapping shapes.
//! Run with `cargo test -p darkburn-geom --test perf -- --nocapture` to see timings.

use darkburn_geom::FillRule;
use darkburn_geom::boolean::union;
use darkburn_geom::kurbo::{BezPath, Circle, Rect, Shape};
use darkburn_geom::offset::{Join, OffsetOptions, OffsetSide, kerf_offset, offset};
use std::time::Instant;

/// 2,000 shapes on a 600 x 400 mm bed: alternating circles and rectangles that overlap
/// their neighbours.
pub fn shapes() -> Vec<BezPath> {
    let mut v = Vec::with_capacity(2000);
    for i in 0..2000 {
        let col = (i % 50) as f64;
        let row = (i / 50) as f64;
        let (x, y) = (col * 12.0, row * 10.0);
        if i % 2 == 0 {
            v.push(Circle::new((x, y), 7.0).to_path(1e-3));
        } else {
            v.push(Rect::new(x - 6.0, y - 4.0, x + 6.0, y + 4.0).to_path(0.1));
        }
    }
    v
}

#[test]
fn union_and_offset_2000_shapes() {
    let s = shapes();
    let t = Instant::now();
    let u = union(&s, FillRule::EvenOdd, 0.01);
    let t_union = t.elapsed();
    assert!(!u.is_empty());

    let t = Instant::now();
    let o = offset(&s, &OffsetOptions { distance: 0.5, side: OffsetSide::Outward, join: Join::Round, tol: 0.01, ..Default::default() });
    let t_offset = t.elapsed();
    assert!(!o.outward.is_empty());

    let t = Instant::now();
    let k: usize = s.iter().map(|p| kerf_offset(p, 0.1, 0.01).elements().len()).sum();
    let t_kerf = t.elapsed();
    assert!(k > 0);

    println!("2000 shapes: union {t_union:?}, offset {t_offset:?}, kerf (each) {t_kerf:?}");
    // Generous bound so slow CI machines and debug builds pass.
    assert!(t_union.as_secs_f64() < 30.0 && t_offset.as_secs_f64() < 30.0 && t_kerf.as_secs_f64() < 30.0);
}
