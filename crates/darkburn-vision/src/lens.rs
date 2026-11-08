//! Camera lens model: pinhole plus radial distortion (standard or fisheye), undistortion of points and images.

use crate::imageops::remap_bilinear;
use image::{ImageBuffer, Pixel};
use kurbo::Point;
use serde::{Deserialize, Serialize};

/// Which distortion model the coefficients belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LensKind {
    /// Brown radial model, `k[0..3]` are k1, k2, k3.
    #[default]
    Standard,
    /// Equidistant fisheye, `θd = θ(1 + k1 θ² + k2 θ⁴ + k3 θ⁶ + k4 θ⁸)`.
    Fisheye,
}

/// Pinhole intrinsics plus radial distortion, in pixels of the original (distorted) image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LensModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: LensKind,
    pub width: u32,
    pub height: u32,
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    #[serde(default)]
    pub k: [f64; 4],
}

impl LensModel {
    /// A lens with no distortion and a centred principal point.
    pub fn ideal(width: u32, height: u32, focal_px: f64) -> Self {
        Self {
            name: String::new(),
            kind: LensKind::Standard,
            width,
            height,
            fx: focal_px,
            fy: focal_px,
            cx: (width as f64 - 1.0) / 2.0,
            cy: (height as f64 - 1.0) / 2.0,
            k: [0.0; 4],
        }
    }

    /// Distort normalised ideal coordinates `(x/z, y/z)`.
    pub fn distort_normalized(&self, x: f64, y: f64) -> (f64, f64) {
        match self.kind {
            LensKind::Standard => {
                let r2 = x * x + y * y;
                let f = 1.0 + self.k[0] * r2 + self.k[1] * r2 * r2 + self.k[2] * r2 * r2 * r2;
                (x * f, y * f)
            }
            LensKind::Fisheye => {
                let r = (x * x + y * y).sqrt();
                if r < 1e-12 {
                    return (x, y);
                }
                let th = r.atan();
                let t2 = th * th;
                let thd = th * (1.0 + t2 * (self.k[0] + t2 * (self.k[1] + t2 * (self.k[2] + t2 * self.k[3]))));
                let s = thd / r;
                (x * s, y * s)
            }
        }
    }

    /// Invert [`distort_normalized`](Self::distort_normalized).
    pub fn undistort_normalized(&self, xd: f64, yd: f64) -> (f64, f64) {
        match self.kind {
            LensKind::Standard => {
                // Newton on the radius keeps this stable for strong barrel distortion.
                let rd = (xd * xd + yd * yd).sqrt();
                if rd < 1e-12 {
                    return (xd, yd);
                }
                let k = &self.k;
                let g = |r: f64| {
                    let r2 = r * r;
                    r * (1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2)
                };
                let dg = |r: f64| {
                    let r2 = r * r;
                    1.0 + 3.0 * k[0] * r2 + 5.0 * k[1] * r2 * r2 + 7.0 * k[2] * r2 * r2 * r2
                };
                let mut r = rd;
                for _ in 0..30 {
                    let d = dg(r);
                    if d.abs() < 1e-9 {
                        break;
                    }
                    let step = (g(r) - rd) / d;
                    r -= step;
                    if step.abs() < 1e-14 {
                        break;
                    }
                }
                let s = r / rd;
                (xd * s, yd * s)
            }
            LensKind::Fisheye => {
                let rd = (xd * xd + yd * yd).sqrt();
                if rd < 1e-12 {
                    return (xd, yd);
                }
                let k = &self.k;
                let f = |t: f64| {
                    let t2 = t * t;
                    t * (1.0 + t2 * (k[0] + t2 * (k[1] + t2 * (k[2] + t2 * k[3]))))
                };
                let df = |t: f64| {
                    let t2 = t * t;
                    1.0 + t2 * (3.0 * k[0] + t2 * (5.0 * k[1] + t2 * (7.0 * k[2] + t2 * 9.0 * k[3])))
                };
                let mut t = rd;
                for _ in 0..30 {
                    let d = df(t);
                    if d.abs() < 1e-9 {
                        break;
                    }
                    let step = (f(t) - rd) / d;
                    t -= step;
                    if step.abs() < 1e-14 {
                        break;
                    }
                }
                let t = t.clamp(0.0, 1.5);
                let r = t.tan();
                let s = r / rd;
                (xd * s, yd * s)
            }
        }
    }

    /// Project a camera-space point (z forward) into distorted pixel coordinates.
    pub fn project(&self, x: f64, y: f64, z: f64) -> Option<Point> {
        if z <= 1e-9 {
            return None;
        }
        let (xd, yd) = self.distort_normalized(x / z, y / z);
        Some(Point::new(self.fx * xd + self.cx, self.fy * yd + self.cy))
    }

    /// Distorted pixel → ideal normalised coordinates.
    pub fn pixel_to_normalized(&self, p: Point) -> (f64, f64) {
        self.undistort_normalized((p.x - self.cx) / self.fx, (p.y - self.cy) / self.fy)
    }

    /// Undistort a pixel position. The result is in pixels of a pinhole camera with the same `fx, fy, cx, cy`.
    pub fn undistort_point(&self, p: Point) -> Point {
        self.undistort_point_with(p, 1.0)
    }

    /// As [`undistort_point`](Self::undistort_point) with the output focal length multiplied by `focal_scale`
    /// (below 1 zooms out so fisheye images keep their edges).
    pub fn undistort_point_with(&self, p: Point, focal_scale: f64) -> Point {
        let (x, y) = self.pixel_to_normalized(p);
        Point::new(self.fx * focal_scale * x + self.cx, self.fy * focal_scale * y + self.cy)
    }

    /// Distort a pixel position of the undistorted image back to the original image.
    pub fn distort_point(&self, p: Point) -> Point {
        self.distort_point_with(p, 1.0)
    }

    pub fn distort_point_with(&self, p: Point, focal_scale: f64) -> Point {
        let x = (p.x - self.cx) / (self.fx * focal_scale);
        let y = (p.y - self.cy) / (self.fy * focal_scale);
        let (xd, yd) = self.distort_normalized(x, y);
        Point::new(self.fx * xd + self.cx, self.fy * yd + self.cy)
    }

    pub fn undistort_points(&self, pts: &[Point]) -> Vec<Point> {
        pts.iter().map(|p| self.undistort_point(*p)).collect()
    }

    /// Undistort a whole image (same size as the model's, or any size: the model is scaled to the image).
    pub fn undistort_image<P: Pixel<Subpixel = u8>>(&self, src: &ImageBuffer<P, Vec<u8>>, focal_scale: f64) -> ImageBuffer<P, Vec<u8>> {
        let sx = src.width() as f64 / self.width.max(1) as f64;
        let sy = src.height() as f64 / self.height.max(1) as f64;
        remap_bilinear(src, src.width(), src.height(), |ox, oy| {
            let p = self.distort_point_with(Point::new(ox / sx, oy / sy), focal_scale);
            Some((p.x * sx, p.y * sy))
        })
    }
}

/// A named camera with a ready-made lens model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LensPreset {
    pub camera_name: String,
    pub model: LensModel,
}

/// User-extensible list of lens presets. Ships empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LensPresets {
    pub entries: Vec<LensPreset>,
}

impl LensPresets {
    /// Built-in presets: none yet. Users add their own (for example from a saved calibration).
    pub fn builtin() -> Self {
        Self::default()
    }

    /// Add or replace the preset for a camera name.
    pub fn add(&mut self, camera_name: impl Into<String>, model: LensModel) {
        let camera_name = camera_name.into();
        match self.entries.iter_mut().find(|e| e.camera_name == camera_name) {
            Some(e) => e.model = model,
            None => self.entries.push(LensPreset { camera_name, model }),
        }
    }

    pub fn remove(&mut self, camera_name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.camera_name != camera_name);
        self.entries.len() != before
    }

    /// Case-insensitive lookup by camera name.
    pub fn find(&self, camera_name: &str) -> Option<&LensModel> {
        self.entries.iter().find(|e| e.camera_name.eq_ignore_ascii_case(camera_name)).map(|e| &e.model)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GrayImage;

    fn barrel() -> LensModel {
        LensModel { k: [-0.25, 0.07, 0.0, 0.0], ..LensModel::ideal(640, 480, 500.0) }
    }

    fn fisheye() -> LensModel {
        LensModel { kind: LensKind::Fisheye, k: [-0.02, 0.005, 0.0, 0.0], ..LensModel::ideal(640, 480, 300.0) }
    }

    #[test]
    fn standard_roundtrip() {
        let l = barrel();
        for p in [Point::new(10.0, 20.0), Point::new(600.0, 400.0), Point::new(320.0, 240.0)] {
            let u = l.undistort_point(p);
            let d = l.distort_point(u);
            assert!((d - p).hypot() < 1e-6, "{p:?} -> {d:?}");
        }
    }

    #[test]
    fn fisheye_roundtrip() {
        let l = fisheye();
        for p in [Point::new(10.0, 20.0), Point::new(600.0, 400.0), Point::new(100.0, 400.0)] {
            let u = l.undistort_point(p);
            let d = l.distort_point(u);
            assert!((d - p).hypot() < 1e-6, "{p:?} -> {d:?}");
        }
    }

    #[test]
    fn centre_is_fixed() {
        let l = barrel();
        let c = Point::new(l.cx, l.cy);
        assert!((l.undistort_point(c) - c).hypot() < 1e-9);
    }

    #[test]
    fn straightens_a_line() {
        // A straight line distorted by barrel distortion becomes straight again after undistortion.
        let l = barrel();
        let pts: Vec<Point> = (0..9).map(|i| l.distort_point(Point::new(40.0 + i as f64 * 70.0, 60.0))).collect();
        assert!((pts[0].y - pts[4].y).abs() > 1.0, "distortion should bend the line");
        let und = l.undistort_points(&pts);
        for p in &und {
            assert!((p.y - 60.0).abs() < 1e-6);
        }
    }

    #[test]
    fn undistort_image_moves_pixels_toward_ideal() {
        let l = barrel();
        let mut img = GrayImage::new(640, 480);
        // draw the distorted image of a straight horizontal line at ideal y = 60
        for i in 0..=640 {
            let p = l.distort_point(Point::new(i as f64, 60.0));
            if p.x >= 0.0 && p.x < 640.0 && p.y >= 0.0 && p.y < 480.0 {
                for dy in 0..2 {
                    img.put_pixel(p.x as u32, (p.y as u32 + dy).min(479), image::Luma([255]));
                }
            }
        }
        let out = l.undistort_image(&img, 1.0);
        // in the output the line sits near row 60 across the width
        let mut max_dev = 0.0f64;
        for x in 80..560 {
            let mut best = None;
            for y in 0..480 {
                if out.get_pixel(x, y)[0] > 100 {
                    best = Some(y);
                    break;
                }
            }
            if let Some(y) = best {
                max_dev = max_dev.max((y as f64 - 60.0).abs());
            }
        }
        assert!(max_dev <= 2.5, "dev {max_dev}");
    }

    #[test]
    fn presets_extensible_and_serde() {
        let mut p = LensPresets::builtin();
        assert!(p.entries.is_empty());
        p.add("Cam A", barrel());
        p.add("cam a", fisheye());
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.find("CAM A").unwrap().kind, LensKind::Standard);
        let back = LensPresets::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        assert!(p.remove("Cam A"));
        assert!(!p.remove("none"));
    }
}
