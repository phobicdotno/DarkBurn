//! DarkBurn vision and workpiece-setup maths: camera lens calibration and alignment, rotary mapping,
//! print and cut registration and test-pattern generators. Pure maths and data, no device access.
#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop)]

pub mod calibrate;
pub mod camera;
pub mod capture;
pub mod centre;
pub mod cylinder;
pub mod detect;
pub mod feeder;
pub mod homography;
pub mod imageops;
pub mod lens;
pub mod linalg;
pub mod printcut;
pub mod rotary;
pub mod strokefont;
pub mod testpatterns;
pub mod transform2d;

#[cfg(test)]
mod testutil;
