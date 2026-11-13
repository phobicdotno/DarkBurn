//! Image modes: turning a grey image into a power map on the layer's scan grid.

use kurbo::Affine;
use serde::{Deserialize, Serialize};

use crate::adjust::{Adjustments, gaussian_blur};
use crate::dither::{ErrorKernel, bayer_matrix, error_diffuse, halftone, newsprint_tile, screen};
use crate::edt::distance_transform_sq;
use crate::gray::{GrayF, PowerMap, fast_to_linear};
use crate::resample::{Resample, ScanGrid, composite_over_white, resample_to_grid};

/// How an image is turned into laser power.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum ImageMode {
    /// On where the image is darker than `threshold` (`0..=1`). With a `ramp_length_mm` above
    /// zero, power rises from zero at the edge of each burnt area to full power that far in
    /// (for stamps), giving an 8-bit map.
    Threshold {
        threshold: f32,
        ramp_length_mm: f64,
    },
    /// 8 × 8 Bayer ordered dither.
    Ordered,
    Atkinson,
    /// Floyd–Steinberg error diffusion (the "Dither" mode).
    FloydSteinberg,
    /// Jarvis–Judice–Ninke error diffusion.
    #[default]
    Jarvis,
    Stucki,
    /// Fine 45° clustered-dot screen.
    Newsprint,
    /// Round dots on a screen of `cells_per_inch` rotated by `angle_deg`.
    Halftone {
        cells_per_inch: f64,
        angle_deg: f64,
    },
    /// High-pass filter that keeps lines and edges (handwriting, line art), dithered.
    /// `radius_px` is the size of detail kept, `gain` the line strength.
    Sketch {
        radius_px: f32,
        gain: f32,
    },
    /// Power follows grey level (8-bit map; the scan maps it between min and max power).
    Grayscale,
    /// The image is already 1-bit: sampled nearest, no adjustments, on where dark.
    PassThrough,
}

impl ImageMode {
    /// True when the mode yields only on/off pixels.
    pub fn is_binary(&self) -> bool {
        match self {
            Self::Threshold { ramp_length_mm, .. } => *ramp_length_mm <= 0.0,
            Self::Grayscale => false,
            _ => true,
        }
    }
}

/// Layer image settings that shape the scan grid and the mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    pub mode: ImageMode,
    /// Distance between scanlines and between dots, mm (`25.4 / DPI`).
    pub line_interval_mm: f64,
    /// Scan direction, degrees counter-clockwise from +X.
    pub scan_angle_deg: f64,
    /// Layer-level negative (combined with the image's own negative).
    pub negative: bool,
    /// Dither in linear light, so dot coverage matches the image's true brightness.
    /// When off, dithering follows the sRGB-encoded values (lighter mid-tones).
    pub linear_light: bool,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self { mode: ImageMode::default(), line_interval_mm: 0.1, scan_angle_deg: 0.0, negative: false, linear_light: true }
    }
}

/// An image processed for one layer: its power map and where it sits on the scan lattice.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedImage {
    pub grid: ScanGrid,
    pub map: PowerMap,
}

impl ProcessedImage {
    /// Map pixel to machine mm.
    pub fn pixel_to_mm(&self) -> Affine {
        self.grid.pixel_to_mm()
    }
}

/// Runs the full image pipeline for one image: resample onto the scan grid, adjust, apply
/// the negative, composite transparency and the area outside the image to white, then apply
/// the image mode.
pub fn process_image(src: &GrayF, image_to_mm: Affine, adjust: &Adjustments, settings: &ImageSettings) -> ProcessedImage {
    let grid = ScanGrid::for_image(src.width, src.height, image_to_mm, settings.line_interval_mm, settings.scan_angle_deg);
    let pass_through = matches!(settings.mode, ImageMode::PassThrough);
    let resample = if pass_through { Resample::Nearest } else { Resample::Area };
    let (mut img, coverage) = resample_to_grid(src, image_to_mm, &grid, resample);
    let negative = settings.negative ^ adjust.negative;
    if pass_through {
        if negative {
            img.data.iter_mut().for_each(|v| *v = 1.0 - *v);
        }
    } else {
        let adj = Adjustments { negative, ..adjust.clone() };
        adj.apply(&mut img);
    }
    composite_over_white(&mut img, &coverage);
    let map = apply_mode(&img, &grid, settings);
    ProcessedImage { grid, map }
}

fn tone(img: &GrayF, linear: bool) -> Vec<f32> {
    if linear { img.data.iter().map(|&v| 1.0 - fast_to_linear(v)).collect() } else { img.data.iter().map(|&v| 1.0 - v).collect() }
}

fn apply_mode(img: &GrayF, grid: &ScanGrid, settings: &ImageSettings) -> PowerMap {
    let (w, h) = (img.width, img.height);
    let gx0 = grid.col0;
    // Only the phase of the screen: a row number pinned at the end of its range (a picture
    // absurdly far away) may wrap here.
    let gy0 = grid.row_top.wrapping_neg();
    let linear = settings.linear_light;
    let binary = |data: Vec<u8>| PowerMap { width: w, height: h, data, binary: true };
    match &settings.mode {
        ImageMode::Threshold { threshold, ramp_length_mm } => {
            let on: Vec<bool> = img.data.iter().map(|&v| v < *threshold).collect();
            if *ramp_length_mm <= 0.0 {
                return binary(on.iter().map(|&o| if o { 255 } else { 0 }).collect());
            }
            let ramp_px = (*ramp_length_mm / grid.interval_mm) as f32;
            let off: Vec<bool> = on.iter().map(|&o| !o).collect();
            let dist = distance_transform_sq(&off, w, h);
            let data = on
                .iter()
                .zip(&dist)
                .map(|(&o, &d2)| {
                    if !o {
                        return 0;
                    }
                    let p = ((d2.sqrt() - 0.5) / ramp_px).clamp(0.0, 1.0);
                    ((p * 255.0).round() as u8).max(1)
                })
                .collect();
            PowerMap { width: w, height: h, data, binary: false }
        }
        ImageMode::Ordered => binary(screen(&tone(img, linear), w, h, &bayer_matrix(8), 8, gx0, gy0)),
        ImageMode::Newsprint => binary(screen(&tone(img, linear), w, h, &newsprint_tile(), 8, gx0, gy0)),
        ImageMode::Atkinson => binary(error_diffuse(&tone(img, linear), w, h, ErrorKernel::Atkinson, false)),
        ImageMode::FloydSteinberg => binary(error_diffuse(&tone(img, linear), w, h, ErrorKernel::FloydSteinberg, true)),
        ImageMode::Jarvis => binary(error_diffuse(&tone(img, linear), w, h, ErrorKernel::JarvisJudiceNinke, true)),
        ImageMode::Stucki => binary(error_diffuse(&tone(img, linear), w, h, ErrorKernel::Stucki, true)),
        ImageMode::Halftone { cells_per_inch, angle_deg } => {
            let cell_px = 25.4 / cells_per_inch.max(1e-3) / grid.interval_mm;
            binary(halftone(&tone(img, linear), w, h, cell_px, *angle_deg, gx0, gy0))
        }
        ImageMode::Sketch { radius_px, gain } => {
            let blurred = gaussian_blur(img, radius_px.max(0.3));
            // Positive where a pixel is darker than its surroundings: lines and dark edges.
            let t: Vec<f32> = img.data.iter().zip(&blurred.data).map(|(&v, &b)| ((b - v) * gain).clamp(0.0, 1.0)).collect();
            binary(error_diffuse(&t, w, h, ErrorKernel::FloydSteinberg, true))
        }
        ImageMode::Grayscale => PowerMap {
            width: w,
            height: h,
            data: img.data.iter().map(|&v| ((1.0 - v.clamp(0.0, 1.0)) * 255.0).round() as u8).collect(),
            binary: false,
        },
        ImageMode::PassThrough => binary(img.data.iter().map(|&v| if v < 0.5 { 255 } else { 0 }).collect()),
    }
}

/// Dot width correction on a binary map, for previews: every horizontal run of on-pixels
/// loses `dot_px` pixels split between its ends, but keeps at least one pixel. (The scan
/// itself corrects in millimetres, see `ScanSettings::dot_width_mm`.)
pub fn erode_rows(map: &PowerMap, dot_px: f64) -> PowerMap {
    let n = dot_px.round().max(0.0) as usize;
    let mut out = map.clone();
    if n == 0 {
        return out;
    }
    let (left, right) = (n / 2, n - n / 2);
    let w = map.width as usize;
    for y in 0..map.height as usize {
        let row = &mut out.data[y * w..(y + 1) * w];
        let mut x = 0;
        while x < w {
            if row[x] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < w && row[x] != 0 {
                x += 1;
            }
            let len = x - start;
            let v = row[start];
            row[start..x].iter_mut().for_each(|p| *p = 0);
            if len > n {
                row[start + left..x - right].iter_mut().for_each(|p| *p = v);
            } else {
                row[start + (len - 1) / 2] = v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement;
    use kurbo::{Point, Size};

    fn gradient() -> GrayF {
        GrayF::from_fn(200, 40, |x, _| x as f32 / 199.0)
    }

    #[test]
    fn transparent_pixels_never_burn_even_when_negative_or_darkened() {
        // Left half opaque white, right half fully transparent.
        let rgba = image::RgbaImage::from_fn(8, 4, |x, _| if x < 4 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        let src = GrayF::from_dynamic(&image::DynamicImage::ImageRgba8(rgba));
        let t = placement(8, 4, Point::ZERO, Size::new(0.8, 0.4));
        for (negative, brightness, resample_pass_through) in [(true, 0.0, false), (false, -100.0, false), (true, 0.0, true)] {
            let mode = if resample_pass_through { ImageMode::PassThrough } else { ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 } };
            let s = ImageSettings { mode, negative, ..Default::default() };
            let p = process_image(&src, t, &Adjustments { brightness, ..Default::default() }, &s);
            assert_eq!((p.map.width, p.map.height), (8, 4));
            for y in 0..4 {
                for x in 0..8 {
                    assert_eq!(p.map.get(x, y) != 0, x < 4, "negative {negative}, brightness {brightness}, at {x},{y}");
                }
            }
        }
        // An opaque pixel next to a transparent one keeps its own tone up to the edge: black
        // beside nothing stays black, it does not fade to white over half a pixel.
        let rgba = image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        let src = GrayF::from_dynamic(&image::DynamicImage::ImageRgba8(rgba));
        let s = ImageSettings { mode: ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 }, ..Default::default() };
        let p = process_image(&src, placement(2, 1, Point::ZERO, Size::new(2.0, 0.1)), &Adjustments::default(), &s);
        assert_eq!(p.map.width, 20);
        let on: Vec<bool> = (0..20).map(|x| p.map.get(x, 0) != 0).collect();
        assert_eq!(on, (0..20).map(|x| x < 10).collect::<Vec<_>>(), "the black pixel burns its whole millimetre and nothing more");
        // Without transparency nothing is kept.
        assert!(
            GrayF::from_dynamic(&image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 255])))).clear.is_empty()
        );
    }

    fn run(mode: ImageMode) -> ProcessedImage {
        let src = gradient();
        let t = placement(200, 40, Point::ZERO, Size::new(20.0, 4.0));
        process_image(&src, t, &Adjustments::default(), &ImageSettings { mode, ..Default::default() })
    }

    #[test]
    fn every_mode_keeps_white_empty_and_black_full() {
        let modes = [
            ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 0.0 },
            ImageMode::Ordered,
            ImageMode::Atkinson,
            ImageMode::FloydSteinberg,
            ImageMode::Jarvis,
            ImageMode::Stucki,
            ImageMode::Newsprint,
            ImageMode::Halftone { cells_per_inch: 50.0, angle_deg: 45.0 },
            ImageMode::Grayscale,
            ImageMode::PassThrough,
        ];
        for mode in modes {
            let p = run(mode.clone());
            assert_eq!((p.map.width, p.map.height), (200, 40));
            assert_eq!(p.map.binary, mode.is_binary());
            // Rightmost column is white, leftmost is black.
            for y in 0..40 {
                assert_eq!(p.map.get(199, y), 0, "{mode:?}");
                if !matches!(mode, ImageMode::Halftone { .. }) {
                    assert_eq!(p.map.get(0, y), 255, "{mode:?}");
                }
            }
        }
    }

    #[test]
    fn linear_light_dithers_darker_midtones() {
        let src = GrayF::new(64, 64, 0.5);
        let t = placement(64, 64, Point::ZERO, Size::new(6.4, 6.4));
        let cov = |linear| {
            let s = ImageSettings { mode: ImageMode::FloydSteinberg, linear_light: linear, ..Default::default() };
            let p = process_image(&src, t, &Adjustments::default(), &s);
            p.map.on_count() as f32 / (64.0 * 64.0)
        };
        assert!((cov(true) - 0.786).abs() < 0.02);
        assert!((cov(false) - 0.5).abs() < 0.02);
    }

    #[test]
    fn negative_does_not_burn_outside_rotated_image() {
        let src = GrayF::new(20, 20, 1.0);
        let t = placement(20, 20, Point::ZERO, Size::new(2.0, 2.0));
        let s = ImageSettings { mode: ImageMode::FloydSteinberg, scan_angle_deg: 30.0, negative: true, ..Default::default() };
        let p = process_image(&src, t, &Adjustments::default(), &s);
        let on = p.map.on_count() as f64 * 0.01;
        assert!((on - 4.0).abs() < 0.5, "{on}");
        // The grid's corners are outside the image and stay off.
        assert_eq!(p.map.get(0, 0), 0);
    }

    #[test]
    fn threshold_ramp_rises_from_edges() {
        let src = GrayF::from_fn(41, 41, |x, y| if (5..36).contains(&x) && (5..36).contains(&y) { 0.0 } else { 1.0 });
        let t = placement(41, 41, Point::ZERO, Size::new(4.1, 4.1));
        let s = ImageSettings { mode: ImageMode::Threshold { threshold: 0.5, ramp_length_mm: 1.0 }, ..Default::default() };
        let p = process_image(&src, t, &Adjustments::default(), &s);
        assert!(!p.map.binary);
        assert_eq!(p.map.get(2, 20), 0);
        let edge = p.map.get(5, 20);
        let mid = p.map.get(10, 20);
        let centre = p.map.get(20, 20);
        assert!(edge > 0 && edge < 30, "{edge}");
        assert!(mid > edge && mid < 255, "{mid}");
        assert_eq!(centre, 255);
    }

    #[test]
    fn pass_through_keeps_dots() {
        let src = GrayF::from_fn(30, 30, |x, y| if (x * 3 + y) % 4 == 0 { 0.0 } else { 1.0 });
        let t = placement(30, 30, Point::ZERO, Size::new(3.0, 3.0));
        let s = ImageSettings { mode: ImageMode::PassThrough, ..Default::default() };
        let adj = Adjustments { brightness: 100.0, ..Default::default() };
        let p = process_image(&src, t, &adj, &s);
        for y in 0..30 {
            for x in 0..30 {
                assert_eq!(p.map.get(x, y) == 255, (x * 3 + y) % 4 == 0);
            }
        }
    }

    #[test]
    fn sketch_keeps_lines_drops_flat_areas() {
        let src = GrayF::from_fn(60, 60, |x, _| {
            if x == 30 {
                0.1
            } else if x < 15 {
                0.4
            } else {
                0.9
            }
        });
        let t = placement(60, 60, Point::ZERO, Size::new(6.0, 6.0));
        let s = ImageSettings { mode: ImageMode::Sketch { radius_px: 2.0, gain: 4.0 }, ..Default::default() };
        let p = process_image(&src, t, &Adjustments::default(), &s);
        let col = |x| (0..60).filter(|&y| p.map.get(x, y) != 0).count();
        assert!(col(30) > 50);
        assert_eq!(col(5), 0);
        assert_eq!(col(45), 0);
    }

    #[test]
    fn grayscale_levels() {
        let p = run(ImageMode::Grayscale);
        assert_eq!(p.map.get(0, 0), 255);
        assert!((i32::from(p.map.get(100, 0)) - 127).abs() <= 3);
    }

    #[test]
    fn erode_rows_shrinks_runs() {
        let mut m = PowerMap::new(12, 1, true);
        m.data = vec![0, 255, 255, 255, 255, 255, 0, 255, 0, 255, 255, 0];
        let e = erode_rows(&m, 2.0);
        assert_eq!(e.data, vec![0, 0, 255, 255, 255, 0, 0, 255, 0, 255, 0, 0]);
    }

    #[test]
    fn mode_serde() {
        let m = ImageMode::Halftone { cells_per_inch: 40.0, angle_deg: 30.0 };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("Halftone"));
        assert_eq!(serde_json::from_str::<ImageMode>(&s).unwrap(), m);
    }
}
