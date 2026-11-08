//! Speed check: a 3000 × 3000 photo-sized image through dithering. Runs in release builds
//! only (`cargo test -p darkburn-raster --release --test perf -- --nocapture`).

use std::time::Instant;

use darkburn_raster::*;
use kurbo::{Point, Size};

#[test]
#[cfg_attr(debug_assertions, ignore = "timing is only meaningful in release builds")]
fn dither_3000_square_under_a_second() {
    let n = 3000u32;
    let src = GrayF::from_fn(n, n, |x, y| ((x as f32 * 0.013).sin() * (y as f32 * 0.007).cos() * 0.5 + 0.5).clamp(0.0, 1.0));
    let t = placement(n, n, Point::ZERO, Size::new(300.0, 300.0));
    let tone: Vec<f32> = src.data.iter().map(|v| 1.0 - v).collect();

    let start = Instant::now();
    let out = error_diffuse(&tone, n, n, ErrorKernel::JarvisJudiceNinke, true);
    let dither_only = start.elapsed();
    assert_eq!(out.len(), (n * n) as usize);

    let start = Instant::now();
    let p = process_image(
        &src,
        t,
        &Adjustments::default(),
        &ImageSettings { mode: ImageMode::FloydSteinberg, line_interval_mm: 0.1, ..Default::default() },
    );
    let full = start.elapsed();
    assert_eq!((p.map.width, p.map.height), (n, n));

    println!("3000x3000 Jarvis dither alone: {dither_only:?}; full pipeline (resample + Floyd-Steinberg): {full:?}");
    assert!(dither_only.as_secs_f64() < 0.5, "{dither_only:?}");
    assert!(full.as_secs_f64() < 1.0, "{full:?}");
}
