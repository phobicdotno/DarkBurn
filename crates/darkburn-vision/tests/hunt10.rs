//! Bug hunt 10: regressions for the vision crate.

use darkburn_vision::camera::{AlignError, AlignmentTarget, CameraAlignment, ImageOrientation};
use darkburn_vision::imageops::sample_bilinear;
use image::GrayImage;
use kurbo::Point;

/// A picture with no pixels has nothing to sample, wherever you ask; it used to panic at the very edge.
#[test]
fn sampling_an_empty_image_gives_nothing_instead_of_panicking() {
    for (w, h) in [(0, 0), (0, 5), (5, 0)] {
        let img = GrayImage::new(w, h);
        for (x, y) in [(-0.5, -0.5), (0.0, 0.0), (-0.5, 2.0), (2.0, -0.5)] {
            assert_eq!(sample_bilinear(&img, x, y), None, "{w}x{h} at ({x}, {y})");
        }
    }
}

/// Three targets whose bed positions sit on one line squash the bed onto that line: the camera
/// image could never be drawn on the bed, so the alignment must be refused, as it is with four.
#[test]
fn three_target_alignment_with_bed_points_on_one_line_is_refused() {
    let targets = [
        AlignmentTarget { image: Point::new(100.0, 100.0), bed: Point::new(10.0, 10.0) },
        AlignmentTarget { image: Point::new(500.0, 120.0), bed: Point::new(50.0, 10.0) },
        AlignmentTarget { image: Point::new(300.0, 400.0), bed: Point::new(90.0, 10.0) },
    ];
    let r = CameraAlignment::solve(&targets, ImageOrientation::default(), (640, 480));
    assert_eq!(r.err(), Some(AlignError::Degenerate));
}

/// A good three-target alignment still works (Y flips between picture and bed, which is normal).
#[test]
fn three_target_alignment_still_accepts_a_good_fit() {
    let targets = [
        AlignmentTarget { image: Point::new(0.0, 0.0), bed: Point::new(0.0, 100.0) },
        AlignmentTarget { image: Point::new(100.0, 0.0), bed: Point::new(50.0, 100.0) },
        AlignmentTarget { image: Point::new(0.0, 100.0), bed: Point::new(0.0, 50.0) },
    ];
    let a = CameraAlignment::solve(&targets, ImageOrientation::default(), (200, 200)).unwrap();
    let p = a.bed_to_image(Point::new(25.0, 75.0)).unwrap();
    assert!((p - Point::new(50.0, 50.0)).hypot() < 1e-9, "{p:?}");
}

fn grid(cols: usize, rows: usize, spacing: f64) -> Vec<Point> {
    let mut v = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            v.push(Point::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    v
}

/// Three captures of a flat target square-on and never moved say nothing about the focal length
/// (near and small looks the same as far and big). The calibration must be refused, not guessed.
#[test]
fn calibration_from_untilted_identical_views_is_refused() {
    use darkburn_vision::calibrate::{CalibError, CalibOptions, calibrate};
    let plane = grid(7, 5, 25.0);
    let view: Vec<Point> = plane.iter().map(|p| Point::new(200.0 + 4.0 * p.x, 150.0 + 4.0 * p.y)).collect();
    let r = calibrate(&plane, &[view.clone(), view.clone(), view], (1280, 720), CalibOptions::default());
    match r {
        Err(e) => assert_eq!(e, CalibError::Degenerate),
        Ok(res) => panic!("accepted: {:?} rms {}", res.model, res.rms_px),
    }
}

/// Print and cut, full affine: three marks measured on one line fit "exactly" with a map that squashes
/// the whole design flat onto that line. That is a mis-measure and must be refused, not cut.
#[test]
fn print_and_cut_full_affine_with_measured_marks_on_one_line_is_refused() {
    use darkburn_vision::printcut::{PnCError, PnCMode, fit_registration};
    let design = [Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(0.0, 100.0)];
    let measured = [Point::new(10.0, 10.0), Point::new(110.0, 10.0), Point::new(60.0, 10.0)];
    let r = fit_registration(&design, &measured, PnCMode::FullAffine);
    assert_eq!(r.err(), Some(PnCError::Degenerate));
}

/// Tiny or empty pictures (a camera that sent nothing) find no pattern; they must not panic.
#[test]
fn pattern_detection_on_tiny_images_finds_nothing() {
    use darkburn_vision::detect::{PatternKind, PatternSpec, detect_checkerboard_corners, detect_circle_blobs, detect_pattern, otsu_threshold};
    for (w, h) in [(0, 0), (1, 1), (0, 7), (7, 0), (2, 2), (3, 1)] {
        let img = GrayImage::new(w, h);
        let _ = otsu_threshold(&img);
        assert!(detect_circle_blobs(&img, true).is_empty());
        let _ = detect_checkerboard_corners(&img);
        for kind in [PatternKind::Checkerboard, PatternKind::CircleGrid { dark_on_light: true }] {
            let spec = PatternSpec { kind, cols: 7, rows: 5, spacing_mm: 25.0 };
            assert!(detect_pattern(&img, &spec).is_none(), "{w}x{h}");
        }
    }
}
