//! Golden-image tests. Inputs and expected outputs live in `tests/data/`.
//!
//! To regenerate after an intended change, run with `DARKBURN_BLESS=1` and review the PNGs.

use std::path::PathBuf;

use darkburn_raster::*;
use image::{DynamicImage, GrayImage, Rgba, RgbaImage};
use kurbo::{Affine, Point, Rect, Shape, Size};

fn data(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("data").join(name)
}

fn bless() -> bool {
    std::env::var_os("DARKBURN_BLESS").is_some()
}

/// 96 × 64: a horizontal grey ramp; the bottom quarter fades to transparent.
fn make_gradient() -> RgbaImage {
    RgbaImage::from_fn(96, 64, |x, y| {
        let v = (x * 255 / 95) as u8;
        let a = if y < 48 { 255 } else { (255 - (y - 48) * 255 / 15) as u8 };
        Rgba([v, v, v, a])
    })
}

/// 80 × 80: a disc, a square with a square hole, and a 2 px diagonal line, black on white.
fn make_shapes() -> RgbaImage {
    RgbaImage::from_fn(80, 80, |x, y| {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let disc = (fx - 22.0).hypot(fy - 22.0) < 14.0;
        let square = (44..72).contains(&x) && (8..36).contains(&y) && !((52..64).contains(&x) && (16..28).contains(&y));
        let line = (fx - fy + 30.0).abs() < 1.2 && (44.0..76.0).contains(&fy);
        if disc || square || line { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
    })
}

fn input(name: &str, make: fn() -> RgbaImage) -> DynamicImage {
    // Tests run in parallel and share the input files.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = data(name);
    if bless() || !path.exists() {
        make().save(&path).unwrap();
    }
    image::open(&path).unwrap()
}

/// Compares with a golden 8-bit image. Allows `tol` difference per pixel and at most
/// `max_bad` pixels beyond it.
fn check_golden(name: &str, actual: &GrayImage, tol: u8, max_bad: usize) {
    let path = data(name);
    if bless() || !path.exists() {
        actual.save(&path).unwrap();
        if !bless() {
            panic!("golden {name} was missing and has been written; review it and run again");
        }
        return;
    }
    let expected = image::open(&path).unwrap().to_luma8();
    assert_eq!(expected.dimensions(), actual.dimensions(), "{name}: size");
    let bad = expected.as_raw().iter().zip(actual.as_raw()).filter(|(a, b)| a.abs_diff(**b) > tol).count();
    if bad > max_bad {
        let out = data(&format!("{name}.actual.png"));
        actual.save(&out).unwrap();
        panic!("{name}: {bad} pixels differ (allowed {max_bad}); actual written to {}", out.display());
    }
}

fn gradient_settings(mode: ImageMode, angle: f64) -> ImageSettings {
    ImageSettings { mode, line_interval_mm: 0.1, scan_angle_deg: angle, ..Default::default() }
}

fn place_gradient() -> Affine {
    // 96 × 64 px at 0.1 mm per pixel, bottom-left at (1, 2) mm.
    placement(96, 64, Point::new(1.0, 2.0), Size::new(9.6, 6.4))
}

#[test]
fn golden_image_modes() {
    let src = GrayF::from_dynamic(&input("gradient.png", make_gradient));
    let modes: [(&str, ImageMode); 12] = [
        ("threshold", ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 }),
        ("threshold_ramp", ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.5 }),
        ("ordered", ImageMode::Ordered),
        ("atkinson", ImageMode::Atkinson),
        ("floyd", ImageMode::FloydSteinberg),
        ("jarvis", ImageMode::Jarvis),
        ("stucki", ImageMode::Stucki),
        ("newsprint", ImageMode::Newsprint),
        ("halftone", ImageMode::Halftone { cells_per_inch: 40.0, angle_deg: 45.0 }),
        ("sketch", ImageMode::Sketch { radius_px: 2.0, gain: 4.0 }),
        ("grayscale", ImageMode::Grayscale),
        ("passthrough", ImageMode::PassThrough),
    ];
    for (name, mode) in modes {
        let p = process_image(&src, place_gradient(), &Adjustments::default(), &gradient_settings(mode, 0.0));
        assert_eq!((p.map.width, p.map.height), (96, 64), "{name}");
        // White and fully transparent areas never burn.
        for y in 0..p.map.height {
            assert_eq!(p.map.get(95, y), 0, "{name}");
        }
        for x in 0..p.map.width {
            assert_eq!(p.map.get(x, 63), 0, "{name}: transparent row");
        }
        let tol = if p.map.binary { 0 } else { 1 };
        check_golden(&format!("golden_{name}.png"), &p.map.to_preview(), tol, 0);
    }
}

#[test]
fn golden_adjusted_and_angled() {
    let src = GrayF::from_dynamic(&input("gradient.png", make_gradient));
    let adj = Adjustments { brightness: 10.0, contrast: 20.0, gamma: 1.2, enhance_amount: 0.5, enhance_radius: 1.0, ..Default::default() };
    let p = process_image(&src, place_gradient(), &adj, &gradient_settings(ImageMode::Jarvis, 0.0));
    check_golden("golden_adjusted_jarvis.png", &p.map.to_preview(), 0, 0);

    let p = process_image(&src, place_gradient(), &Adjustments::default(), &gradient_settings(ImageMode::Jarvis, 30.0));
    check_golden("golden_angled_30_jarvis.png", &p.map.to_preview(), 0, 0);
    // Resampled at 0.2 mm: half the resolution, area-averaged in linear light.
    let p = process_image(
        &src,
        place_gradient(),
        &Adjustments::default(),
        &ImageSettings { line_interval_mm: 0.2, ..gradient_settings(ImageMode::Grayscale, 0.0) },
    );
    assert_eq!((p.map.width, p.map.height), (48, 32));
    check_golden("golden_resampled_grayscale.png", &p.map.to_preview(), 1, 0);
}

#[test]
fn angled_scan_lines_follow_the_angle() {
    let src = GrayF::new(50, 50, 0.0);
    let t = placement(50, 50, Point::ZERO, Size::new(5.0, 5.0));
    let p = process_image(&src, t, &Adjustments::default(), &gradient_settings(ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 }, 30.0));
    let plans = scan_plans(&[&p], &ScanSettings { overscan_mm: 0.0, blank_skip_mm: None, ..Default::default() });
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    // Every burnt point, mapped back to the machine frame, lies inside the 5 mm square
    // (within one interval of its edges).
    for line in &plan.lines {
        for span in &line.spans {
            for run in &span.runs {
                for x in [run.x0, run.x1] {
                    let p = plan.point_mm(line, x);
                    assert!(p.x > -0.15 && p.x < 5.15 && p.y > -0.15 && p.y < 5.15, "{p:?}");
                }
            }
        }
    }
    // Total burnt length × interval ≈ area.
    let area = plan.burn_length() * plan.interval_mm;
    assert!((area - 25.0).abs() < 0.8, "{area}");
}

#[test]
fn golden_trace_shapes() {
    let src = GrayF::from_dynamic(&input("shapes.png", make_shapes));
    let paths = trace_image(&src, Affine::IDENTITY, &TraceSettings::default());
    // Disc, square outline, square hole, line.
    assert_eq!(paths.len(), 4, "{paths:?}");
    // Rasterising the trace (1 px per mm, same frame flipped to Y up) reproduces the input.
    let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 80.0]);
    let flipped: Vec<_> = paths.iter().map(|p| flip * p.clone()).collect();
    let r = rasterize_paths(&flipped, PaintStyle::Fill { even_odd: true }, 25.4, Some(Rect::new(0.0, 0.0, 80.0, 80.0)), 1 << 20).unwrap();
    let raster = r.image.to_luma8();
    let input = src.to_luma8();
    let mismatched = raster.as_raw().iter().zip(input.as_raw()).filter(|(a, b)| a.abs_diff(**b) > 128).count();
    assert!(mismatched < 40, "{mismatched}");
    check_golden("golden_trace_shapes.png", &raster, 8, 20);
}

#[test]
fn golden_convert_to_bitmap_and_mask() {
    let shapes = [kurbo::Circle::new((5.0, 5.0), 4.0).to_path(0.01), Rect::new(10.0, 1.0, 18.0, 9.0).to_path(0.01)];
    let r = rasterize_paths(&shapes, PaintStyle::Fill { even_odd: false }, 254.0, None, 1 << 24).unwrap();
    check_golden("golden_convert_fill.png", &r.image.to_luma8(), 8, 10);
    let r = rasterize_paths(&shapes, PaintStyle::Stroke { width_mm: 0.5 }, 127.0, None, 1 << 24).unwrap();
    check_golden("golden_convert_stroke.png", &r.image.to_luma8(), 8, 10);

    let img = input("gradient.png", make_gradient).to_rgba8();
    let clip = [kurbo::Circle::new((5.8, 5.2), 3.0).to_path(0.01)];
    let masked = apply_mask(&img, place_gradient(), &clip, false);
    let flat = DynamicImage::ImageRgb8(flatten_alpha(&masked)).to_luma8();
    check_golden("golden_mask.png", &flat, 2, 10);
}
