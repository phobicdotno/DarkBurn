//! Feeder setup maths (OTHER-04): how far one feed moves the material, calibrating it from a measurement,
//! the delay to wait for a feed, and the plan for running a long piece through the work area in windows.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FeederError {
    InvalidSize,
    InvalidMeasurement,
}

impl std::fmt::Display for FeederError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidSize => "sizes must be above zero and overlap must be smaller than the window",
            Self::InvalidMeasurement => "measured distance must be above zero",
        })
    }
}
impl std::error::Error for FeederError {}

/// Distance the feed axis must cover so the material advances by `window_height - overlap`.
pub fn distance_per_feed(window_height: f64, overlap: f64) -> Result<f64, FeederError> {
    if !(window_height > 0.0) || overlap < 0.0 || overlap >= window_height {
        return Err(FeederError::InvalidSize);
    }
    Ok(window_height - overlap)
}

/// Material advance per full turn of a feed roller.
pub fn distance_per_roller_turn(roller_diameter: f64) -> Result<f64, FeederError> {
    if !(roller_diameter > 0.0) {
        return Err(FeederError::InvalidSize);
    }
    Ok(PI * roller_diameter)
}

/// Correct a steps-per-mm (or distance scale) value: the feeder was told to move `commanded` mm and the material moved `measured` mm.
pub fn corrected_steps_per_mm(current: f64, commanded: f64, measured: f64) -> Result<f64, FeederError> {
    if !(commanded > 0.0 && current > 0.0) {
        return Err(FeederError::InvalidSize);
    }
    if !(measured > 0.0) {
        return Err(FeederError::InvalidMeasurement);
    }
    Ok(current * commanded / measured)
}

/// Milliseconds to wait after commanding a feed of `distance` mm at `speed_mm_s`, plus a settle time.
pub fn feed_delay_ms(distance: f64, speed_mm_s: f64, settle_ms: f64) -> Result<f64, FeederError> {
    if !(speed_mm_s > 0.0) || distance < 0.0 || settle_ms < 0.0 {
        return Err(FeederError::InvalidSize);
    }
    Ok(distance / speed_mm_s * 1000.0 + settle_ms)
}

/// Motor steps for a roller feed of `distance` mm.
pub fn steps_for_roller_feed(distance: f64, roller_diameter: f64, steps_per_rotation: f64) -> Result<f64, FeederError> {
    if !(steps_per_rotation > 0.0) {
        return Err(FeederError::InvalidSize);
    }
    Ok(distance / distance_per_roller_turn(roller_diameter)? * steps_per_rotation)
}

/// One window of a long piece.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeedWindow {
    pub index: usize,
    /// Where this window starts along the material, mm.
    pub start: f64,
    /// Material to advance after burning this window (0 for the last).
    pub feed_after: f64,
}

/// The most windows a plan can have.
pub const MAX_WINDOWS: usize = 100_000;

/// Split a piece of `total_length` into windows of `window_height` that overlap by `overlap`.
pub fn plan_windows(total_length: f64, window_height: f64, overlap: f64) -> Result<Vec<FeedWindow>, FeederError> {
    let advance = distance_per_feed(window_height, overlap)?;
    if !(total_length > 0.0) {
        return Err(FeederError::InvalidSize);
    }
    let count = if total_length <= window_height { 1.0 } else { (((total_length - overlap) / advance) - 1e-9).ceil() };
    // More windows than any roll has: a typing slip, and too many to list.
    if !(count <= MAX_WINDOWS as f64) {
        return Err(FeederError::InvalidSize);
    }
    let n = count as usize;
    Ok((0..n).map(|i| FeedWindow { index: i, start: i as f64 * advance, feed_after: if i + 1 < n { advance } else { 0.0 } }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_with_an_absurd_number_of_windows_is_refused_not_a_crash() {
        assert_eq!(plan_windows(1e300, 300.0, 10.0).unwrap_err(), FeederError::InvalidSize);
        assert_eq!(plan_windows(f64::INFINITY, 300.0, 10.0).unwrap_err(), FeederError::InvalidSize);
        assert_eq!(plan_windows(1e9, 1.0, 0.0).unwrap_err(), FeederError::InvalidSize);
        assert_eq!(plan_windows(f64::NAN, 300.0, 10.0).unwrap_err(), FeederError::InvalidSize);
        // a long roll is still fine
        assert_eq!(plan_windows(100_000.0, 300.0, 10.0).unwrap().len(), 345);
    }

    #[test]
    fn feed_distance_and_errors() {
        assert_eq!(distance_per_feed(300.0, 10.0).unwrap(), 290.0);
        assert_eq!(distance_per_feed(300.0, 300.0).unwrap_err(), FeederError::InvalidSize);
        assert_eq!(distance_per_feed(0.0, 0.0).unwrap_err(), FeederError::InvalidSize);
        assert!((distance_per_roller_turn(20.0).unwrap() - 62.83185307).abs() < 1e-6);
    }

    #[test]
    fn calibration_from_measurement() {
        // asked for 100 mm, got 98 mm → need 2.04 % more steps
        let v = corrected_steps_per_mm(80.0, 100.0, 98.0).unwrap();
        assert!((v - 81.632653).abs() < 1e-5);
        // applying the corrected value makes the move come out right
        let moved = 98.0 * v / 80.0;
        assert!((moved - 100.0).abs() < 1e-9);
        assert_eq!(corrected_steps_per_mm(80.0, 100.0, 0.0).unwrap_err(), FeederError::InvalidMeasurement);
    }

    #[test]
    fn delay_and_steps() {
        assert_eq!(feed_delay_ms(100.0, 50.0, 250.0).unwrap(), 2250.0);
        assert!(feed_delay_ms(1.0, 0.0, 0.0).is_err());
        let s = steps_for_roller_feed(PI * 20.0, 20.0, 3200.0).unwrap();
        assert!((s - 3200.0).abs() < 1e-9);
    }

    #[test]
    fn window_plan_covers_the_piece() {
        let w = plan_windows(1000.0, 300.0, 10.0).unwrap();
        // advance 290: windows start at 0, 290, 580, 870 → last ends at 1170 ≥ 1000, third ends at 880 < 1000
        assert_eq!(w.len(), 4);
        assert_eq!(w[1].start, 290.0);
        assert_eq!(w[3].feed_after, 0.0);
        assert_eq!(w[0].feed_after, 290.0);
        let last_end = w.last().unwrap().start + 300.0;
        assert!(last_end >= 1000.0 && w[w.len() - 2].start + 300.0 < 1000.0);
        assert_eq!(plan_windows(250.0, 300.0, 10.0).unwrap().len(), 1);
        assert_eq!(plan_windows(300.0, 300.0, 10.0).unwrap().len(), 1);
        assert!(plan_windows(0.0, 300.0, 10.0).is_err());
    }
}
