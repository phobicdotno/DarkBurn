//! Bug hunt 10b: more regressions for the vision crate (rotary, cylinder, feeder, exposure).

use darkburn_vision::camera::suggest_exposure;
use darkburn_vision::cylinder::{CylinderAxis, CylinderCorrection, CylinderError, CylinderKind};
use darkburn_vision::feeder::{FeederError, distance_per_feed};
use darkburn_vision::rotary::{RotaryError, RotaryParams, split_by_slice};
use kurbo::{BezPath, Point};

/// An exposure range typed the wrong way round (max below min) must still give a value in it, not crash.
#[test]
fn exposure_suggestion_with_a_reversed_range_does_not_panic() {
    let v = suggest_exposure(5.0, 0.5, 0.5, (10.0, 1.0));
    assert!((1.0..=10.0).contains(&v), "{v}");
    let v = suggest_exposure(5.0, 0.5, 0.5, (f64::NAN, 10.0));
    assert!(v.is_finite(), "{v}");
}

/// A slice size far smaller than the design asks for an absurd number of slices: refuse it instead of crashing.
#[test]
fn rotary_slices_of_absurdly_small_size_are_refused_not_a_crash() {
    let params = RotaryParams { enabled: true, ..RotaryParams::default() };
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((10.0, 100.0));
    assert_eq!(split_by_slice(std::slice::from_ref(&p), 1e-300, &params).unwrap_err(), RotaryError::InvalidSliceSize);
    // a sensible size still works
    assert_eq!(split_by_slice(std::slice::from_ref(&p), 30.0, &params).unwrap().len(), 4);
}

/// An overlap that is not a number is not a valid overlap.
#[test]
fn feed_distance_with_nan_overlap_is_refused() {
    assert_eq!(distance_per_feed(300.0, f64::NAN).unwrap_err(), FeederError::InvalidSize);
    assert_eq!(distance_per_feed(f64::INFINITY, 0.0).unwrap_err(), FeederError::InvalidSize);
}

/// An infinite radius or a centre line that is not a number gives NaN positions: refuse it.
#[test]
fn cylinder_with_infinite_radius_or_nan_centre_is_refused() {
    let base = CylinderCorrection { radius: 30.0, kind: CylinderKind::Convex, axis: CylinderAxis::AlongX, centre: 100.0 };
    let inf = CylinderCorrection { radius: f64::INFINITY, ..base };
    assert_eq!(inf.map_point(Point::new(0.0, 110.0)).unwrap_err(), CylinderError::InvalidRadius);
    let nan_centre = CylinderCorrection { centre: f64::NAN, ..base };
    let r = nan_centre.map_point(Point::new(0.0, 110.0));
    assert!(r.is_err(), "{r:?}");
    assert!(base.map_point(Point::new(0.0, 110.0)).is_ok());
}
