//! Image engraving for DarkBurn.
//!
//! The pipeline is:
//!
//! 1. [`GrayF::from_dynamic`] turns any decoded image into a grey image. Transparent pixels
//!    become white, colour is reduced to luminance in linear light.
//! 2. [`process_image`] resamples it onto the layer's scan grid (line interval and scan angle,
//!    in millimetres), applies the [`Adjustments`] and the [`ImageMode`], and returns a
//!    [`ProcessedImage`] holding a 1-bit or 8-bit [`PowerMap`].
//! 3. [`scan_plans`] turns one or more processed images into [`ScanPlan`]s: scanlines with
//!    on-runs, power, direction, overscan and blank-skip spans, ready for the motion engine.
//!
//! Separately, [`trace_image`] turns an image into vector paths, [`rasterize_paths`] turns
//! vectors into a bitmap, [`apply_mask`] clips an image with vectors, and [`PresetLibrary`]
//! stores Adjust Image presets.
//!
//! Geometry is in millimetres, Y up, using `kurbo` types. Image pixel space is x right,
//! y down, with pixel `(0, 0)` covering `[0, 1) × [0, 1)`.

mod adjust;
mod dither;
mod dpi;
mod edt;
mod gray;
mod mask;
mod preset;
mod process;
mod rasterize;
mod resample;
mod scan;
mod trace;

pub use adjust::{Adjustments, gaussian_blur};
pub use dither::{ErrorKernel, bayer_matrix, error_diffuse, halftone, newsprint_tile, screen};
pub use dpi::{dpi_to_interval_mm, interval_mm_to_dpi, read_dpi};
pub use edt::distance_transform_sq;
pub use gray::{GrayF, PowerMap, linear_to_srgb, srgb_to_linear};
pub use mask::{apply_mask, flatten_alpha};
pub use preset::{AdjustPreset, PresetError, PresetLibrary};
pub use process::{ImageMode, ImageSettings, ProcessedImage, erode_rows, process_image};
pub use rasterize::{PaintStyle, RasterError, RasterizedPaths, rasterize_paths};
pub use resample::{Resample, ScanGrid, resample_to_grid};
pub use scan::{ScanLine, ScanPlan, ScanRun, ScanSettings, ScanSpan, scan_plans};
pub use trace::{TraceMode, TraceSettings, trace_image};

/// The affine map from image pixels to millimetres for an unrotated image whose bottom-left
/// corner sits at `origin_mm` and which is `size_mm` large.
pub fn placement(width_px: u32, height_px: u32, origin_mm: kurbo::Point, size_mm: kurbo::Size) -> kurbo::Affine {
    let sx = size_mm.width / f64::from(width_px.max(1));
    let sy = size_mm.height / f64::from(height_px.max(1));
    kurbo::Affine::new([sx, 0.0, 0.0, -sy, origin_mm.x, origin_mm.y + size_mm.height])
}
