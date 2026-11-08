//! Timings on a 3000 × 3000 synthetic photo: `cargo run -p darkburn-raster --release --example bench`.

use std::time::Instant;

use darkburn_raster::*;
use kurbo::{Point, Size};

fn main() {
    let n = 3000u32;
    let src = GrayF::from_fn(n, n, |x, y| ((x as f32 * 0.013).sin() * (y as f32 * 0.007).cos() * 0.5 + 0.5).clamp(0.0, 1.0));
    let t = placement(n, n, Point::ZERO, Size::new(300.0, 300.0));
    let modes = [
        ("threshold", ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 }),
        ("ordered", ImageMode::Ordered),
        ("atkinson", ImageMode::Atkinson),
        ("floyd-steinberg", ImageMode::FloydSteinberg),
        ("jarvis", ImageMode::Jarvis),
        ("stucki", ImageMode::Stucki),
        ("newsprint", ImageMode::Newsprint),
        ("halftone", ImageMode::Halftone { cells_per_inch: 60.0, angle_deg: 45.0 }),
        ("sketch", ImageMode::Sketch { radius_px: 2.0, gain: 4.0 }),
        ("grayscale", ImageMode::Grayscale),
    ];
    let mut last = None;
    for (name, mode) in modes {
        let s = ImageSettings { mode, line_interval_mm: 0.1, ..Default::default() };
        let start = Instant::now();
        let p = process_image(&src, t, &Adjustments::default(), &s);
        println!("{name:>16}: {:?} (resample + mode)", start.elapsed());
        last = Some(p);
    }
    let s = ImageSettings { mode: ImageMode::Jarvis, line_interval_mm: 0.1, scan_angle_deg: 30.0, ..Default::default() };
    let start = Instant::now();
    let angled = process_image(&src, t, &Adjustments::default(), &s);
    println!("{:>16}: {:?} ({} × {} grid)", "jarvis at 30°", start.elapsed(), angled.map.width, angled.map.height);

    let adj = Adjustments { contrast: 20.0, gamma: 1.2, enhance_amount: 1.0, enhance_radius: 1.5, enhance_denoise: 0.02, ..Default::default() };
    let start = Instant::now();
    let _ = process_image(&src, t, &adj, &ImageSettings { mode: ImageMode::Jarvis, ..Default::default() });
    println!("{:>16}: {:?}", "adjusted jarvis", start.elapsed());

    let p = last.expect("ran");
    let start = Instant::now();
    let plans = scan_plans(&[&p], &ScanSettings::default());
    let lines: usize = plans.iter().map(|p| p.lines.len()).sum();
    println!("{:>16}: {:?} ({lines} lines)", "scan_plans", start.elapsed());

    let start = Instant::now();
    let paths = trace_image(&src, t, &TraceSettings::default());
    println!("{:>16}: {:?} ({} paths)", "trace", start.elapsed(), paths.len());
}
