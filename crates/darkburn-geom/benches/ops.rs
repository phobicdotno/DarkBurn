//! Simple timing bench (no external harness): `cargo bench -p darkburn-geom`.

use darkburn_geom::FillRule;
use darkburn_geom::boolean::{difference, union};
use darkburn_geom::kurbo::{BezPath, Circle, Rect, Shape};
use darkburn_geom::offset::{Join, OffsetOptions, OffsetSide, kerf_offset, offset};
use std::time::Instant;

fn shapes(n: usize) -> Vec<BezPath> {
    (0..n)
        .map(|i| {
            let (x, y) = ((i % 50) as f64 * 12.0, (i / 50) as f64 * 10.0);
            if i % 2 == 0 { Circle::new((x, y), 7.0).to_path(1e-3) } else { Rect::new(x - 6.0, y - 4.0, x + 6.0, y + 4.0).to_path(0.1) }
        })
        .collect()
}

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) {
    let _ = f();
    let t = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(f());
    }
    println!("{label:<32} {:>10.2?} per run", t.elapsed() / runs);
}

fn main() {
    let s = shapes(2000);
    let half = s.len() / 2;
    time("union 2000", 5, || union(&s, FillRule::EvenOdd, 0.01));
    time("difference 1000 - 1000", 5, || difference(&s[..half], &s[half..], FillRule::EvenOdd, 0.01));
    for (name, join) in [("round", Join::Round), ("miter", Join::Miter { limit: 4.0 }), ("bevel", Join::Bevel)] {
        let o = OffsetOptions { distance: 0.5, side: OffsetSide::Both, join, tol: 0.01, ..Default::default() };
        time(&format!("offset both 2000 ({name})"), 5, || offset(&s, &o));
    }
    time("kerf 2000 (each)", 5, || s.iter().map(|p| kerf_offset(p, 0.1, 0.01)).count());
}
