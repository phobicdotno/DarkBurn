//! Bug hunt 10c: regressions for the vision crate (cylinder path length, rotary test turn, camera settings file).

use darkburn_vision::camera::{CaptureSettings, OverlayAdjust};
use darkburn_vision::cylinder::{CylinderAxis, CylinderCorrection, CylinderKind, MAX_SEGMENTS_PER_LINE};
use darkburn_vision::rotary::{RotaryError, RotaryParams, test_rotation_plan};
use kurbo::{BezPath, Point};

/// A line along the cylinder axis a million billion mm long must not be split into a million billion points:
/// the step is widened so the line stays within the cap, and the answer comes back quickly.
#[test]
fn cylinder_path_of_absurd_length_is_split_into_a_bounded_number_of_points() {
    let cyl = CylinderCorrection { radius: 30.0, kind: CylinderKind::Convex, axis: CylinderAxis::AlongX, centre: 100.0 };
    let mut p = BezPath::new();
    p.move_to((0.0, 100.0));
    p.line_to((1e15, 100.0));
    let started = std::time::Instant::now();
    let out = cyl.map_path(&p, 1.0).unwrap();
    assert!(started.elapsed().as_secs() < 10, "took {:?}", started.elapsed());
    let n = out.elements().len();
    assert!(n <= MAX_SEGMENTS_PER_LINE + 1, "{n} points");
    // the line still ends where it should
    assert_eq!(out.elements().last().and_then(|e| e.end_point()), Some(Point::new(1e15, 100.0)));
    // an ordinary line is still split at the asked step
    let mut q = BezPath::new();
    q.move_to((0.0, 100.0));
    q.line_to((10.0, 100.0));
    assert_eq!(cyl.map_path(&q, 1.0).unwrap().elements().len(), 11);
}

/// A test turn of NaN or infinite revolutions is refused instead of giving NaN or infinite moves.
#[test]
fn rotary_test_turn_with_nan_or_infinite_revolutions_is_refused() {
    let params = RotaryParams { enabled: true, ..RotaryParams::default() };
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(test_rotation_plan(&params, bad).unwrap_err(), RotaryError::InvalidRevolutions, "{bad}");
    }
    let ok = test_rotation_plan(&params, 1.0).unwrap();
    assert!(ok.moves.iter().all(|m| m.distance.is_finite()));
}

/// One NaN inside a nested part of the camera settings must not lose the whole file on load.
#[test]
fn camera_settings_with_a_nan_inside_a_nested_part_keep_the_rest_on_load() {
    let s = CaptureSettings {
        camera_name: "Bench cam".into(),
        device_index: Some(2),
        resolution: Some((1920, 1080)),
        exposure: Some(f64::NAN),
        brightness: Some(0.25),
        focal_scale: 1.5,
        overlay: OverlayAdjust { x_shift_mm: f64::NAN, y_shift_mm: 3.0, width_mm: None, height_mm: None },
        crop_to_page: true,
        ..CaptureSettings::default()
    };
    let back = CaptureSettings::from_json(&s.to_json()).expect("one bad number must not lose the whole file");
    assert_eq!(back.camera_name, "Bench cam");
    assert_eq!(back.device_index, Some(2));
    assert_eq!(back.resolution, Some((1920, 1080)));
    assert_eq!(back.brightness, Some(0.25));
    assert_eq!(back.focal_scale, 1.5);
    assert!(back.crop_to_page);
    // the bad numbers fall back to their defaults
    assert_eq!(back.exposure, None);
    assert_eq!(back.overlay, OverlayAdjust::default());
}
