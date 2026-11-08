//! Trace image: bitmap to vector paths. Our own tracer (no potrace code): pixel-edge
//! boundary loops, corner detection and least-squares cubic fitting.

mod contour;
mod fit;

use kurbo::{Affine, BezPath, Point, Rect};
use serde::{Deserialize, Serialize};

use crate::adjust::gaussian_blur;
use crate::gray::GrayF;
use fit::{FitParams, fit_closed, fit_open};

/// What is traced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TraceMode {
    /// Closed outlines around dark areas.
    Outline,
    /// Open centre lines along dark lines and edges (photos to line drawings, handwriting).
    /// `radius_px` is the size of detail kept; `sensitivity` (`0..=1`) picks up fainter edges
    /// as it rises.
    Sketch { radius_px: f32, sensitivity: f32 },
}

/// Trace image settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceSettings {
    /// Pixels darker than this (`0..=255`) are traced (outline mode).
    pub threshold: u8,
    /// Pixels darker than this are left out, so a band of greys can be traced alone.
    pub lower_cutoff: u8,
    /// Specks and holes with fewer pixels than this are ignored.
    pub ignore_smaller_px: u32,
    /// `0` gives straight lines only; higher values round more corners; `1.33` and above
    /// round every corner.
    pub smoothness: f32,
    /// Curve-fitting tolerance in pixels: larger values give fewer nodes.
    pub optimize: f32,
    /// Only pixels whose centre lies inside this rectangle (image pixels) are traced.
    pub clip: Option<Rect>,
    pub mode: TraceMode,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self { threshold: 128, lower_cutoff: 0, ignore_smaller_px: 2, smoothness: 1.0, optimize: 0.5, clip: None, mode: TraceMode::Outline }
    }
}

impl TraceSettings {
    fn fit_params(&self) -> FitParams {
        let s = f64::from(self.smoothness.max(0.0));
        FitParams { corner_deg: if s >= 1.3 { 181.0 } else { 10.0 + 60.0 * s }, tolerance: f64::from(self.optimize).max(0.05), polygonal: s < 0.01 }
    }
}

/// Traces `img` (transparent areas already white, see [`GrayF::from_dynamic`]) and returns
/// paths in mm via `image_to_mm`. Outline mode gives one closed path per boundary: holes
/// wind the other way, so the paths can be combined and filled with either fill rule.
/// Sketch mode gives open paths (and closed ones for rings).
pub fn trace_image(img: &GrayF, image_to_mm: Affine, settings: &TraceSettings) -> Vec<BezPath> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut mask: Vec<bool> = match &settings.mode {
        TraceMode::Outline => {
            let hi = f32::from(settings.threshold) / 255.0;
            let lo = f32::from(settings.lower_cutoff) / 255.0;
            img.data.iter().map(|&v| v < hi && v >= lo).collect()
        }
        TraceMode::Sketch { radius_px, sensitivity } => {
            let blurred = gaussian_blur(img, radius_px.max(0.5));
            let t = 0.02 + 0.3 * (1.0 - sensitivity.clamp(0.0, 1.0));
            let lo = f32::from(settings.lower_cutoff) / 255.0;
            img.data.iter().zip(&blurred.data).map(|(&v, &b)| b - v > t && v >= lo).collect()
        }
    };
    if let Some(clip) = settings.clip {
        for y in 0..h {
            for x in 0..w {
                if !clip.contains(Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)) {
                    mask[(y * w + x) as usize] = false;
                }
            }
        }
    }
    contour::remove_small(&mut mask, w, h, settings.ignore_smaller_px as usize);
    let params = settings.fit_params();
    let mut paths = match settings.mode {
        TraceMode::Outline => contour::trace_loops(&mask, w, h)
            .into_iter()
            .map(|lp| {
                let n = lp.len();
                let mids: Vec<Point> = (0..n)
                    .map(|i| {
                        let (a, b) = (lp[i], lp[(i + 1) % n]);
                        Point::new(f64::from(a.0 + b.0) / 2.0, f64::from(a.1 + b.1) / 2.0)
                    })
                    .collect();
                fit_closed(&mids, &params)
            })
            .collect::<Vec<_>>(),
        TraceMode::Sketch { .. } => {
            contour::thin(&mut mask, w, h);
            let min_len = (settings.ignore_smaller_px as usize).max(2);
            contour::skeleton_chains(&mask, w, h)
                .into_iter()
                .filter(|c| c.len() >= min_len)
                .map(|c| {
                    let pts: Vec<Point> = c.iter().map(|&(x, y)| Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)).collect();
                    if pts.len() > 3 && pts.first() == pts.last() { fit_closed(&pts[..pts.len() - 1], &params) } else { fit_open(&pts, &params) }
                })
                .collect()
        }
    };
    paths.retain(|p| !p.elements().is_empty());
    for p in &mut paths {
        p.apply_affine(image_to_mm);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement;
    use kurbo::{PathEl, Shape, Size};

    fn disc(w: u32, cx: f32, cy: f32, r: f32) -> GrayF {
        GrayF::from_fn(w, w, |x, y| if (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) < r { 0.0 } else { 1.0 })
    }

    #[test]
    fn traces_a_disc_in_mm() {
        let img = disc(100, 50.0, 50.0, 30.0);
        let t = placement(100, 100, Point::new(10.0, 20.0), Size::new(50.0, 50.0));
        let paths = trace_image(&img, t, &TraceSettings::default());
        assert_eq!(paths.len(), 1);
        let p = &paths[0];
        let bb = p.bounding_box();
        assert!((bb.center().x - 35.0).abs() < 0.3 && (bb.center().y - 45.0).abs() < 0.3, "{bb:?}");
        assert!((bb.width() - 30.0).abs() < 0.6, "{bb:?}");
        let area = p.area().abs();
        assert!((area - std::f64::consts::PI * 225.0).abs() < 8.0, "{area}");
        let curves = p.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count();
        assert!(curves <= 16, "{curves}");
    }

    #[test]
    fn square_stays_square_and_hole_found() {
        let img = GrayF::from_fn(40, 40, |x, y| {
            let outer = (5..35).contains(&x) && (5..35).contains(&y);
            let hole = (15..25).contains(&x) && (15..25).contains(&y);
            if outer && !hole { 0.0 } else { 1.0 }
        });
        let paths = trace_image(&img, Affine::IDENTITY, &TraceSettings::default());
        assert_eq!(paths.len(), 2);
        let mut areas: Vec<f64> = paths.iter().map(|p| p.area()).collect();
        areas.sort_by(f64::total_cmp);
        assert!((areas[0].abs() - 900.0).abs() < 1e-6 || (areas[1].abs() - 900.0).abs() < 1e-6, "{areas:?}");
        assert!(areas[0].signum() != areas[1].signum());
        let total: f64 = areas.iter().sum::<f64>().abs();
        assert!((total - 800.0).abs() < 1e-6);
    }

    #[test]
    fn band_trace_and_ignore_small() {
        // Left half dark grey, right half black, plus one black speck.
        let mut img = GrayF::from_fn(40, 20, |x, _| if x < 20 { 0.4 } else { 0.0 });
        img.set(5, 5, 1.0);
        let s = TraceSettings { lower_cutoff: 50, threshold: 128, ignore_smaller_px: 4, ..Default::default() };
        let paths = trace_image(&img, Affine::IDENTITY, &s);
        assert_eq!(paths.len(), 1, "speck hole ignored, black half left out");
        let bb = paths[0].bounding_box();
        assert!(bb.x1 <= 20.0 + 1e-9);
    }

    #[test]
    fn clip_rectangle() {
        let img = GrayF::new(30, 30, 0.0);
        let s = TraceSettings { clip: Some(Rect::new(10.0, 10.0, 20.0, 25.0)), ..Default::default() };
        let paths = trace_image(&img, Affine::IDENTITY, &s);
        assert_eq!(paths.len(), 1);
        assert!((paths[0].area().abs() - 150.0).abs() < 1e-6);
    }

    #[test]
    fn polygonal_when_smoothness_zero() {
        let img = disc(60, 30.0, 30.0, 20.0);
        let s = TraceSettings { smoothness: 0.0, ..Default::default() };
        let paths = trace_image(&img, Affine::IDENTITY, &s);
        assert!(paths[0].elements().iter().all(|e| !matches!(e, PathEl::CurveTo(..))));
    }

    #[test]
    fn sketch_finds_centre_line() {
        // A dark horizontal line 3 px thick on white.
        let img = GrayF::from_fn(60, 30, |x, y| if (14..17).contains(&y) && (5..55).contains(&x) { 0.0 } else { 1.0 });
        let s = TraceSettings { mode: TraceMode::Sketch { radius_px: 3.0, sensitivity: 0.5 }, ..Default::default() };
        let paths = trace_image(&img, Affine::IDENTITY, &s);
        assert!(!paths.is_empty());
        let longest = paths.iter().max_by(|a, b| a.bounding_box().width().total_cmp(&b.bounding_box().width())).unwrap();
        let bb = longest.bounding_box();
        assert!(bb.width() > 40.0, "{bb:?}");
        assert!((bb.center().y - 15.5).abs() < 1.0, "{bb:?}");
    }

    #[test]
    fn transparent_is_not_traced() {
        use image::{DynamicImage, Rgba, RgbaImage};
        let mut rgba = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 0]));
        for y in 5..15 {
            for x in 5..15 {
                rgba.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let g = GrayF::from_dynamic(&DynamicImage::ImageRgba8(rgba));
        let paths = trace_image(&g, Affine::IDENTITY, &TraceSettings::default());
        assert_eq!(paths.len(), 1);
        assert!((paths[0].area().abs() - 100.0).abs() < 1e-6);
    }
}
