//! Camera alignment (image to bed), overlay placement and state, head-camera mosaics, exposure helpers,
//! capture settings import/export and tracing from a capture.

use crate::homography::{Homography, fit_homography};
use crate::imageops::{remap_bilinear, sample_bilinear};
use crate::lens::LensModel;
use crate::transform2d::fit_affine;
use image::{GrayImage, ImageBuffer, Luma, Pixel, Rgba, RgbaImage};
use kurbo::{Affine, Point, Rect};
use serde::{Deserialize, Serialize};

/// Overhead camera or camera mounted on the laser head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraKind {
    #[default]
    Overhead,
    HeadMounted,
}

/// A rectangle on the bed in millimetres (Y up, origin at the machine origin).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl BedRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn centre(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn to_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.x + self.width, self.y + self.height)
    }

    /// Pixel size of an image covering this rectangle at `px_per_mm`.
    pub fn pixel_size(&self, px_per_mm: f64) -> (u32, u32) {
        ((self.width * px_per_mm).round().max(1.0) as u32, (self.height * px_per_mm).round().max(1.0) as u32)
    }
}

/// Camera image orientation fix-ups applied before alignment: rotate by quarter turns (clockwise) then flip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImageOrientation {
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    /// Clockwise quarter turns, 0..=3 (CAM-04 "rotate 90").
    #[serde(default)]
    pub rotate_quarters: u8,
}

impl ImageOrientation {
    pub fn is_identity(&self) -> bool {
        !self.flip_x && !self.flip_y && self.rotate_quarters.is_multiple_of(4)
    }

    pub fn oriented_size(&self, w: u32, h: u32) -> (u32, u32) {
        if !self.rotate_quarters.is_multiple_of(2) { (h, w) } else { (w, h) }
    }

    /// Map a pixel position of the source image to the oriented image.
    pub fn apply_point(&self, p: Point, w: u32, h: u32) -> Point {
        let (mut x, mut y) = (p.x, p.y);
        let (mut cw, mut ch) = (w as f64, h as f64);
        for _ in 0..self.rotate_quarters % 4 {
            // clockwise quarter turn: (x, y) → (h-1-y, x)
            (x, y) = (ch - 1.0 - y, x);
            std::mem::swap(&mut cw, &mut ch);
        }
        if self.flip_x {
            x = cw - 1.0 - x;
        }
        if self.flip_y {
            y = ch - 1.0 - y;
        }
        Point::new(x, y)
    }

    /// Inverse of [`apply_point`](Self::apply_point).
    pub fn invert_point(&self, p: Point, w: u32, h: u32) -> Point {
        let (ow, oh) = self.oriented_size(w, h);
        let (mut x, mut y) = (p.x, p.y);
        if self.flip_y {
            y = oh as f64 - 1.0 - y;
        }
        if self.flip_x {
            x = ow as f64 - 1.0 - x;
        }
        let (mut cw, mut ch) = (ow as f64, oh as f64);
        for _ in 0..self.rotate_quarters % 4 {
            // counter-clockwise quarter turn: (x, y) → (y, w-1-x) with current width
            (x, y) = (y, cw - 1.0 - x);
            std::mem::swap(&mut cw, &mut ch);
        }
        Point::new(x, y)
    }

    pub fn apply_image<P: Pixel<Subpixel = u8>>(&self, img: &ImageBuffer<P, Vec<u8>>) -> ImageBuffer<P, Vec<u8>> {
        if self.is_identity() {
            return img.clone();
        }
        let (w, h) = (img.width(), img.height());
        let (ow, oh) = self.oriented_size(w, h);
        let mut out = ImageBuffer::<P, Vec<u8>>::new(ow, oh);
        for y in 0..h {
            for x in 0..w {
                let q = self.apply_point(Point::new(x as f64, y as f64), w, h);
                out.put_pixel(q.x.round() as u32, q.y.round() as u32, *img.get_pixel(x, y));
            }
        }
        out
    }
}

/// One alignment target: where it appears in the undistorted camera image and where it really is (mm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlignmentTarget {
    pub image: Point,
    pub bed: Point,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AlignError {
    NotEnoughTargets { have: usize },
    Degenerate,
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughTargets { have } => write!(f, "need at least 3 targets (4 for full perspective), got {have}"),
            Self::Degenerate => write!(f, "the targets are collinear or coincident"),
        }
    }
}
impl std::error::Error for AlignError {}

/// Mapping from the (undistorted, oriented) camera image to bed millimetres.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraAlignment {
    #[serde(default)]
    pub orientation: ImageOrientation,
    /// Size of the undistorted source image the targets were measured in.
    pub image_size: (u32, u32),
    /// Oriented undistorted pixels → bed mm.
    pub homography: Homography,
    /// Extra uniform scale already folded into `homography` (CAM-04 "scale").
    #[serde(default = "one")]
    pub scale: f64,
    /// Root mean square target error after the fit, mm.
    pub rms_mm: f64,
}

fn one() -> f64 {
    1.0
}

/// Standard N-target layout: 4 targets at the corners of the work area inset by `margin`, extra ones along the edges.
pub fn default_targets(bed: BedRect, margin: f64, count: usize) -> Vec<Point> {
    let (x0, y0, x1, y1) = (bed.x + margin, bed.y + margin, bed.x + bed.width - margin, bed.y + bed.height - margin);
    let mut v = vec![Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)];
    let extra = [
        Point::new((x0 + x1) / 2.0, y0),
        Point::new(x1, (y0 + y1) / 2.0),
        Point::new((x0 + x1) / 2.0, y1),
        Point::new(x0, (y0 + y1) / 2.0),
        Point::new((x0 + x1) / 2.0, (y0 + y1) / 2.0),
    ];
    for e in extra {
        if v.len() >= count {
            break;
        }
        v.push(e);
    }
    v.truncate(count.max(3));
    v
}

impl CameraAlignment {
    /// Solve from N targets whose `image` positions are in the undistorted, un-oriented camera image.
    /// Four or more targets give a perspective homography refined by least squares; three give an affine map.
    pub fn solve(targets: &[AlignmentTarget], orientation: ImageOrientation, image_size: (u32, u32)) -> Result<Self, AlignError> {
        if targets.len() < 3 {
            return Err(AlignError::NotEnoughTargets { have: targets.len() });
        }
        let src: Vec<Point> = targets.iter().map(|t| orientation.apply_point(t.image, image_size.0, image_size.1)).collect();
        let dst: Vec<Point> = targets.iter().map(|t| t.bed).collect();
        let homography = if targets.len() >= 4 {
            fit_homography(&src, &dst).map_err(|_| AlignError::Degenerate)?
        } else {
            let a = fit_affine(&src, &dst).ok_or(AlignError::Degenerate)?;
            // Bed points on one line squash the picture onto that line: it could never be turned round.
            let c = a.as_coeffs();
            let size = c[..4].iter().map(|v| v * v).sum::<f64>();
            if !(a.determinant().abs() > 1e-9 * size) {
                return Err(AlignError::Degenerate);
            }
            Homography::from_affine(a)
        };
        let rms = homography.rms_error(&src, &dst);
        if !rms.is_finite() {
            return Err(AlignError::Degenerate);
        }
        Ok(Self { orientation, image_size, homography, scale: 1.0, rms_mm: rms })
    }

    /// Fold an extra uniform scale about `centre` (bed mm) into the mapping.
    pub fn apply_scale(&mut self, factor: f64, centre: Point) {
        let a = Affine::translate(centre.to_vec2()) * Affine::scale(factor) * Affine::translate(-centre.to_vec2());
        self.homography = Homography::from_affine(a).then_after(&self.homography);
        self.scale *= factor;
    }

    /// Undistorted source pixel → bed mm.
    pub fn image_to_bed(&self, p: Point) -> Option<Point> {
        self.homography.apply(self.orientation.apply_point(p, self.image_size.0, self.image_size.1))
    }

    /// Bed mm → undistorted source pixel.
    pub fn bed_to_image(&self, p: Point) -> Option<Point> {
        let q = self.homography.inverse()?.apply(p)?;
        Some(self.orientation.invert_point(q, self.image_size.0, self.image_size.1))
    }

    /// Per-target distance between the mapped image position and the true bed position, mm.
    pub fn residuals_mm(&self, targets: &[AlignmentTarget]) -> Vec<f64> {
        targets.iter().map(|t| self.image_to_bed(t.image).map_or(f64::INFINITY, |p| (p - t.bed).hypot())).collect()
    }

    /// Warp an undistorted camera image into a bed-aligned image covering `view` at `px_per_mm` (north up, Y up on the bed).
    pub fn render_bed_image<P: Pixel<Subpixel = u8>>(
        &self,
        undistorted: &ImageBuffer<P, Vec<u8>>,
        view: BedRect,
        px_per_mm: f64,
    ) -> ImageBuffer<P, Vec<u8>> {
        let oriented = self.orientation.apply_image(undistorted);
        let (ow, oh) = view.pixel_size(px_per_mm);
        let inv = self.homography.inverse();
        remap_bilinear(&oriented, ow, oh, |x, y| {
            let bed = Point::new(view.x + (x + 0.5) / px_per_mm, view.y + view.height - (y + 0.5) / px_per_mm);
            inv?.apply(bed).map(|p| (p.x, p.y))
        })
    }
}

/// Where the camera overlay lands on the canvas (CAMP-07 width, height and X/Y shift fields).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct OverlayAdjust {
    pub x_shift_mm: f64,
    pub y_shift_mm: f64,
    /// `None` keeps the bed width.
    pub width_mm: Option<f64>,
    /// `None` keeps the bed height.
    pub height_mm: Option<f64>,
}

impl OverlayAdjust {
    /// Canvas rectangle (mm) the overlay occupies.
    pub fn placement(&self, bed: BedRect) -> Rect {
        let w = self.width_mm.unwrap_or(bed.width);
        let h = self.height_mm.unwrap_or(bed.height);
        let (x0, y0) = (bed.x + self.x_shift_mm, bed.y + self.y_shift_mm);
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// Affine taking overlay image pixels (origin top-left, Y down) to canvas millimetres (Y up).
    pub fn pixel_to_canvas(&self, bed: BedRect, image_w: u32, image_h: u32) -> Affine {
        let r = self.placement(bed);
        let (sx, sy) = (r.width() / image_w.max(1) as f64, r.height() / image_h.max(1) as f64);
        Affine::new([sx, 0.0, 0.0, -sy, r.x0, r.y1])
    }
}

/// Show/fade/colour state of the overlay (CAM-06, CAM-08).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayState {
    pub show: bool,
    /// 0 = fully visible, 1 = fully faded out.
    pub fade: f32,
    /// false shows the camera in black and white.
    pub color: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self { show: true, fade: 0.0, color: true }
    }
}

impl OverlayState {
    pub fn opacity(&self) -> f32 {
        if self.show { (1.0 - self.fade).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// The image as it should be drawn, or `None` when hidden.
    pub fn apply(&self, img: &RgbaImage) -> Option<RgbaImage> {
        let op = self.opacity();
        if op <= 0.0 {
            return None;
        }
        let mut out = img.clone();
        for p in out.pixels_mut() {
            if !self.color {
                let l = luma(p.0[0], p.0[1], p.0[2]).round() as u8;
                p.0[0] = l;
                p.0[1] = l;
                p.0[2] = l;
            }
            p.0[3] = (p.0[3] as f32 * op).round() as u8;
        }
        Some(out)
    }
}

/// Crop a bed-aligned image (covering `covers`) to a page rectangle in mm (CAM-09 "crop to page").
pub fn crop_to_page(img: &RgbaImage, covers: BedRect, px_per_mm: f64, page: Rect) -> RgbaImage {
    let x0 = ((page.x0 - covers.x) * px_per_mm).floor().clamp(0.0, img.width() as f64) as u32;
    let x1 = ((page.x1 - covers.x) * px_per_mm).ceil().clamp(0.0, img.width() as f64) as u32;
    // image rows run top-down
    let y0 = ((covers.y + covers.height - page.y1) * px_per_mm).floor().clamp(0.0, img.height() as f64) as u32;
    let y1 = ((covers.y + covers.height - page.y0) * px_per_mm).ceil().clamp(0.0, img.height() as f64) as u32;
    if x1 <= x0 || y1 <= y0 {
        return RgbaImage::new(1, 1);
    }
    image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image()
}

// ---------------------------------------------------------------- head-mounted camera

/// A mark seen by a head-mounted camera: its raw pixel, where the head was, and where the mark truly is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadMark {
    pub image: Point,
    pub head_pos: Point,
    pub bed: Point,
}

/// Head camera calibration: maps undistorted pixels to millimetres relative to the head position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadCameraCalibration {
    pub lens: Option<LensModel>,
    #[serde(default = "one")]
    pub focal_scale: f64,
    pub image_size: (u32, u32),
    pub to_offset_mm: Homography,
    pub rms_mm: f64,
}

impl HeadCameraCalibration {
    pub fn solve(marks: &[HeadMark], image_size: (u32, u32), lens: Option<LensModel>, focal_scale: f64) -> Result<Self, AlignError> {
        if marks.len() < 4 {
            return Err(AlignError::NotEnoughTargets { have: marks.len() });
        }
        let src: Vec<Point> = marks.iter().map(|m| lens.as_ref().map_or(m.image, |l| l.undistort_point_with(m.image, focal_scale))).collect();
        let dst: Vec<Point> = marks.iter().map(|m| Point::new(m.bed.x - m.head_pos.x, m.bed.y - m.head_pos.y)).collect();
        let h = fit_homography(&src, &dst).map_err(|_| AlignError::Degenerate)?;
        Ok(Self { lens, focal_scale, image_size, rms_mm: h.rms_error(&src, &dst), to_offset_mm: h })
    }

    /// Bed position seen at raw pixel `px` when the head is at `head_pos`.
    pub fn bed_point(&self, head_pos: Point, px: Point) -> Option<Point> {
        let u = self.lens.as_ref().map_or(px, |l| l.undistort_point_with(px, self.focal_scale));
        self.to_offset_mm.apply(u).map(|o| Point::new(head_pos.x + o.x, head_pos.y + o.y))
    }
}

/// One capture taken with the head at a known position.
pub struct MosaicCapture {
    pub head_pos: Point,
    pub image: RgbaImage,
}

/// Stitch head-camera captures into a bed-aligned image covering `view` (weights fade towards each capture's edges).
/// Pixels covered by no capture stay transparent.
pub fn stitch_mosaic(captures: &[MosaicCapture], cal: &HeadCameraCalibration, view: BedRect, px_per_mm: f64) -> RgbaImage {
    let (ow, oh) = view.pixel_size(px_per_mm);
    let prepared: Vec<(RgbaImage, Option<Homography>, Point)> = captures
        .iter()
        .map(|c| {
            let img = match &cal.lens {
                Some(l) => l.undistort_image(&c.image, cal.focal_scale),
                None => c.image.clone(),
            };
            (img, cal.to_offset_mm.inverse(), c.head_pos)
        })
        .collect();
    let mut out = RgbaImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let bed = Point::new(view.x + (x as f64 + 0.5) / px_per_mm, view.y + view.height - (y as f64 + 0.5) / px_per_mm);
            let (mut acc, mut wsum) = ([0.0f64; 4], 0.0f64);
            for (img, inv, head) in &prepared {
                let Some(inv) = inv else { continue };
                let Some(px) = inv.apply(Point::new(bed.x - head.x, bed.y - head.y)) else { continue };
                let (w, h) = (img.width() as f64, img.height() as f64);
                if px.x < 0.0 || px.y < 0.0 || px.x > w - 1.0 || px.y > h - 1.0 {
                    continue;
                }
                let feather = 0.15 * w.min(h);
                let edge = px.x.min(w - 1.0 - px.x).min(px.y).min(h - 1.0 - px.y);
                let wt = (edge / feather).clamp(0.0, 1.0).max(1e-3);
                if let Some(v) = sample_bilinear(img, px.x, px.y) {
                    for c in 0..4 {
                        acc[c] += v[c] as f64 * wt;
                    }
                    wsum += wt;
                }
            }
            if wsum > 0.0 {
                let p = [acc[0] / wsum, acc[1] / wsum, acc[2] / wsum, 255.0];
                out.put_pixel(x, y, Rgba([p[0].round() as u8, p[1].round() as u8, p[2].round() as u8, 255]));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- exposure and brightness

#[inline]
fn luma(r: u8, g: u8, b: u8) -> f32 {
    0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
}

pub fn to_grayscale(img: &RgbaImage) -> GrayImage {
    let mut out = GrayImage::new(img.width(), img.height());
    for (x, y, p) in img.enumerate_pixels() {
        out.put_pixel(x, y, Luma([luma(p.0[0], p.0[1], p.0[2]).round() as u8]));
    }
    out
}

pub fn luma_histogram(img: &RgbaImage) -> [u32; 256] {
    let mut h = [0u32; 256];
    for p in img.pixels() {
        h[luma(p.0[0], p.0[1], p.0[2]).round() as usize] += 1;
    }
    h
}

/// Mean brightness, 0.0 to 1.0.
pub fn mean_luma(img: &RgbaImage) -> f64 {
    let n = (img.width() as f64 * img.height() as f64).max(1.0);
    img.pixels().map(|p| luma(p.0[0], p.0[1], p.0[2]) as f64).sum::<f64>() / n / 255.0
}

/// Brightness and contrast, both in -1.0..=1.0 (0 leaves the image unchanged).
pub fn adjust_brightness_contrast(img: &RgbaImage, brightness: f32, contrast: f32) -> RgbaImage {
    let b = brightness.clamp(-1.0, 1.0);
    let c = contrast.clamp(-1.0, 1.0);
    let factor = if c >= 0.0 { 1.0 / (1.0 - c * 0.99) } else { 1.0 + c };
    let mut out = img.clone();
    for p in out.pixels_mut() {
        for ch in 0..3 {
            let v = p.0[ch] as f32 / 255.0;
            p.0[ch] = (((v - 0.5) * factor + 0.5 + b).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// Next exposure value to try: moves `current` towards a mean brightness of `target` (damped), clamped to `range`.
pub fn suggest_exposure(current: f64, measured_mean: f64, target: f64, range: (f64, f64)) -> f64 {
    let ratio = (target / measured_mean.max(0.01)).clamp(0.25, 4.0);
    // `f64::clamp` panics on a reversed or NaN range; min/max skip a NaN bound.
    let (lo, hi) = (range.0.min(range.1), range.0.max(range.1));
    (current * ratio.powf(0.8)).max(lo).min(hi)
}

// ---------------------------------------------------------------- settings

/// Everything the camera panel remembers (CAM-07 import/export, CAM-09 remember last camera).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureSettings {
    pub version: u32,
    pub camera_name: String,
    #[serde(default)]
    pub kind: CameraKind,
    #[serde(default)]
    pub device_index: Option<u32>,
    #[serde(default)]
    pub resolution: Option<(u32, u32)>,
    #[serde(default)]
    pub exposure: Option<f64>,
    #[serde(default)]
    pub brightness: Option<f64>,
    #[serde(default)]
    pub lens: Option<LensModel>,
    #[serde(default = "one")]
    pub focal_scale: f64,
    #[serde(default)]
    pub alignment: Option<CameraAlignment>,
    #[serde(default)]
    pub head: Option<HeadCameraCalibration>,
    #[serde(default)]
    pub overlay: OverlayAdjust,
    #[serde(default)]
    pub state: OverlayState,
    #[serde(default)]
    pub crop_to_page: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            camera_name: String::new(),
            kind: CameraKind::Overhead,
            device_index: None,
            resolution: None,
            exposure: None,
            brightness: None,
            lens: None,
            focal_scale: 1.0,
            alignment: None,
            head: None,
            overlay: OverlayAdjust::default(),
            state: OverlayState::default(),
            crop_to_page: false,
        }
    }
}

impl CaptureSettings {
    pub const VERSION: u32 = 1;

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

// ---------------------------------------------------------------- trace from capture

/// A grayscale image ready for tracing, with its placement when the camera is aligned.
#[derive(Clone, Debug)]
pub struct TraceImage {
    pub image: GrayImage,
    /// Bed area the image covers and its resolution, when an alignment was applied.
    pub placement: Option<(BedRect, f64)>,
}

/// Convert a capture (or overlay) into a grayscale image for tracing, undistorting with the lens and, when an
/// alignment is given, warping to bed coordinates so the traced shapes land at true size.
pub fn trace_from_capture(
    raw: &RgbaImage,
    lens: Option<&LensModel>,
    focal_scale: f64,
    alignment: Option<(&CameraAlignment, BedRect, f64)>,
) -> TraceImage {
    let undistorted = match lens {
        Some(l) => l.undistort_image(raw, focal_scale),
        None => raw.clone(),
    };
    match alignment {
        Some((al, view, ppm)) => {
            let mut bed_img = al.render_bed_image(&undistorted, view, ppm);
            // Uncovered pixels are transparent; show them as white paper.
            for p in bed_img.pixels_mut() {
                if p.0[3] == 0 {
                    *p = Rgba([255, 255, 255, 255]);
                }
            }
            TraceImage { image: to_grayscale(&bed_img), placement: Some((view, ppm)) }
        }
        None => TraceImage { image: to_grayscale(&undistorted), placement: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truth() -> Homography {
        // image px → mm: scale 0.25 mm/px with mild perspective and a Y flip (image y down)
        Homography { m: [0.25, 0.002, 5.0, 0.003, -0.25, 300.0, 1e-5, 2e-6, 1.0] }
    }

    fn targets(n: usize) -> Vec<AlignmentTarget> {
        let px = [Point::new(100.0, 80.0), Point::new(1100.0, 90.0), Point::new(1120.0, 640.0), Point::new(90.0, 630.0), Point::new(600.0, 350.0)];
        px.iter().take(n).map(|p| AlignmentTarget { image: *p, bed: truth().apply(*p).unwrap() }).collect()
    }

    #[test]
    fn targets_on_one_line_or_clicked_twice_are_refused() {
        // four marks along the front edge of the bed, clicked along one row of the picture
        let line: Vec<AlignmentTarget> = (0..4)
            .map(|i| AlignmentTarget { image: Point::new(100.0 + 300.0 * i as f64, 600.0), bed: Point::new(20.0 + 100.0 * i as f64, 20.0) })
            .collect();
        assert_eq!(CameraAlignment::solve(&line, ImageOrientation::default(), (1280, 720)), Err(AlignError::Degenerate));
        // the same spot clicked for two different targets
        let mut twice = targets(4);
        twice[3].image = twice[2].image;
        assert_eq!(CameraAlignment::solve(&twice, ImageOrientation::default(), (1280, 720)), Err(AlignError::Degenerate));
        // three targets on a line (the affine case)
        assert_eq!(CameraAlignment::solve(&line[..3], ImageOrientation::default(), (1280, 720)), Err(AlignError::Degenerate));
    }

    #[test]
    fn alignment_maps_picture_to_bed_and_back() {
        // 0.5 mm per pixel, picture Y down, bed Y up, picture corner (0, 599) at the bed origin
        let t: Vec<AlignmentTarget> = [(0.0, 599.0), (799.0, 599.0), (799.0, 0.0), (0.0, 0.0)]
            .iter()
            .map(|&(x, y)| AlignmentTarget { image: Point::new(x, y), bed: Point::new(x * 0.5, (599.0 - y) * 0.5) })
            .collect();
        let al = CameraAlignment::solve(&t, ImageOrientation::default(), (800, 600)).unwrap();
        let bed = al.image_to_bed(Point::new(200.0, 499.0)).unwrap();
        assert!((bed - Point::new(100.0, 50.0)).hypot() < 1e-6, "{bed:?}");
        let px = al.bed_to_image(Point::new(100.0, 50.0)).unwrap();
        assert!((px - Point::new(200.0, 499.0)).hypot() < 1e-6, "{px:?}");
    }

    #[test]
    fn alignment_with_four_and_five_targets() {
        for n in [4, 5] {
            let al = CameraAlignment::solve(&targets(n), ImageOrientation::default(), (1280, 720)).unwrap();
            assert!(al.rms_mm < 1e-6, "rms {}", al.rms_mm);
            let probe = Point::new(333.0, 222.0);
            let got = al.image_to_bed(probe).unwrap();
            assert!((got - truth().apply(probe).unwrap()).hypot() < 1e-5);
            let back = al.bed_to_image(got).unwrap();
            assert!((back - probe).hypot() < 1e-5);
        }
    }

    #[test]
    fn three_targets_give_affine() {
        let t: Vec<AlignmentTarget> = [Point::new(0.0, 0.0), Point::new(100.0, 0.0), Point::new(0.0, 100.0)]
            .iter()
            .map(|p| AlignmentTarget { image: *p, bed: Point::new(p.x * 0.5 + 10.0, p.y * 0.5 + 20.0) })
            .collect();
        let al = CameraAlignment::solve(&t, ImageOrientation::default(), (200, 200)).unwrap();
        assert!((al.image_to_bed(Point::new(50.0, 50.0)).unwrap() - Point::new(35.0, 45.0)).hypot() < 1e-9);
        assert_eq!(CameraAlignment::solve(&t[..2], ImageOrientation::default(), (200, 200)).unwrap_err(), AlignError::NotEnoughTargets { have: 2 });
    }

    #[test]
    fn orientation_roundtrip_and_image() {
        let (w, h) = (6u32, 4u32);
        for q in 0..4u8 {
            for (fx, fy) in [(false, false), (true, false), (false, true), (true, true)] {
                let o = ImageOrientation { flip_x: fx, flip_y: fy, rotate_quarters: q };
                let p = Point::new(1.0, 3.0);
                let there = o.apply_point(p, w, h);
                assert!((o.invert_point(there, w, h) - p).hypot() < 1e-12);
                let mut img = GrayImage::new(w, h);
                img.put_pixel(1, 3, Luma([200]));
                let rot = o.apply_image(&img);
                assert_eq!(rot.get_pixel(there.x as u32, there.y as u32)[0], 200);
            }
        }
    }

    #[test]
    fn rotated_camera_is_handled() {
        // camera rotated 90 degrees clockwise: build targets in the rotated image, solve with orientation fix
        let o = ImageOrientation { rotate_quarters: 3, ..Default::default() };
        let (w, h) = (1280u32, 720u32);
        let src_px = [Point::new(100.0, 80.0), Point::new(1100.0, 90.0), Point::new(1120.0, 640.0), Point::new(90.0, 630.0)];
        let t: Vec<AlignmentTarget> = src_px
            .iter()
            .map(|p| {
                let q = o.apply_point(*p, w, h);
                AlignmentTarget { image: *p, bed: Point::new(q.x * 0.3, 400.0 - q.y * 0.3) }
            })
            .collect();
        let al = CameraAlignment::solve(&t, o, (w, h)).unwrap();
        assert!(al.rms_mm < 1e-6);
    }

    #[test]
    fn bed_render_places_features() {
        // Synthetic camera: 1 px = 0.5 mm, bed 100 x 60, camera image 200 x 120, bed y up.
        let mut img = RgbaImage::from_pixel(200, 120, Rgba([255, 255, 255, 255]));
        // black square at bed (20..30, 10..20) mm → px x 40..60, y rows from top: 120-(20/0.5)=80 .. 100
        for y in 80..100 {
            for x in 40..60 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let t: Vec<AlignmentTarget> = [(0.0, 0.0), (100.0, 0.0), (100.0, 60.0), (0.0, 60.0)]
            .iter()
            .map(|&(bx, by)| AlignmentTarget { image: Point::new(bx * 2.0, 120.0 - by * 2.0), bed: Point::new(bx, by) })
            .collect();
        let al = CameraAlignment::solve(&t, ImageOrientation::default(), (200, 120)).unwrap();
        let view = BedRect::new(0.0, 0.0, 100.0, 60.0);
        let out = al.render_bed_image(&img, view, 4.0);
        assert_eq!(out.dimensions(), (400, 240));
        // bed (25, 15) → px (100, 240-60=180)
        assert_eq!(out.get_pixel(100, 180)[0], 0);
        assert_eq!(out.get_pixel(300, 60)[0], 255);
        // scale folding: doubling about the origin moves the square
        let mut al2 = al.clone();
        al2.apply_scale(2.0, Point::new(0.0, 0.0));
        assert!((al2.image_to_bed(Point::new(100.0, 120.0 - 30.0)).unwrap() - Point::new(100.0, 30.0)).hypot() < 1e-6);
        assert!((al2.scale - 2.0).abs() < 1e-12);
        // crop to page
        let page = Rect::new(20.0, 10.0, 30.0, 20.0);
        let c = crop_to_page(&out, view, 4.0, page);
        assert_eq!(c.dimensions(), (40, 40));
        assert_eq!(c.get_pixel(20, 20)[0], 0);
    }

    #[test]
    fn overlay_adjust_and_state() {
        let bed = BedRect::new(0.0, 0.0, 400.0, 300.0);
        let adj = OverlayAdjust { x_shift_mm: 2.0, y_shift_mm: -1.0, width_mm: Some(410.0), height_mm: None };
        let r = adj.placement(bed);
        assert_eq!((r.x0, r.y0, r.width(), r.height()), (2.0, -1.0, 410.0, 300.0));
        let a = adj.pixel_to_canvas(bed, 820, 600);
        assert!((a * Point::new(0.0, 0.0) - Point::new(2.0, 299.0)).hypot() < 1e-9);
        assert!((a * Point::new(820.0, 600.0) - Point::new(412.0, -1.0)).hypot() < 1e-9);

        let img = RgbaImage::from_pixel(2, 2, Rgba([200, 100, 50, 255]));
        let mut st = OverlayState::default();
        assert_eq!(st.apply(&img).unwrap().get_pixel(0, 0).0, [200, 100, 50, 255]);
        st.fade = 0.5;
        st.color = false;
        let v = st.apply(&img).unwrap();
        assert_eq!(v.get_pixel(0, 0).0[3], 128);
        assert_eq!(v.get_pixel(0, 0).0[0], v.get_pixel(0, 0).0[1]);
        st.show = false;
        assert!(st.apply(&img).is_none());
    }

    #[test]
    fn head_camera_mosaic_stitches_two_views() {
        // Head camera sees a 40 x 30 mm patch at 5 px/mm (200x150 px), centred under the head.
        let h = Homography { m: [0.2, 0.0, -20.0, 0.0, -0.2, 15.0, 0.0, 0.0, 1.0] };
        let mk = |head: Point, px: Point| {
            let off = h.apply(px).unwrap();
            HeadMark { image: px, head_pos: head, bed: Point::new(head.x + off.x, head.y + off.y) }
        };
        let hp = Point::new(50.0, 50.0);
        let marks =
            vec![mk(hp, Point::new(10.0, 10.0)), mk(hp, Point::new(190.0, 10.0)), mk(hp, Point::new(190.0, 140.0)), mk(hp, Point::new(10.0, 140.0))];
        let cal = HeadCameraCalibration::solve(&marks, (200, 150), None, 1.0).unwrap();
        assert!(cal.rms_mm < 1e-6);
        assert!((cal.bed_point(Point::new(0.0, 0.0), Point::new(100.0, 75.0)).unwrap() - Point::new(0.0, 0.0)).hypot() < 1e-6);

        // Bed has a gradient in X: colour = x mm * 2. Capture at two head positions.
        let capture = |head: Point| {
            let mut img = RgbaImage::new(200, 150);
            for y in 0..150 {
                for x in 0..200 {
                    let o = h.apply(Point::new(x as f64, y as f64)).unwrap();
                    let bx = head.x + o.x;
                    img.put_pixel(x, y, Rgba([(bx * 2.0).clamp(0.0, 255.0) as u8, 0, 0, 255]));
                }
            }
            img
        };
        let caps = vec![
            MosaicCapture { head_pos: Point::new(30.0, 30.0), image: capture(Point::new(30.0, 30.0)) },
            MosaicCapture { head_pos: Point::new(60.0, 30.0), image: capture(Point::new(60.0, 30.0)) },
        ];
        let view = BedRect::new(10.0, 15.0, 80.0, 30.0);
        let out = stitch_mosaic(&caps, &cal, view, 2.0);
        assert_eq!(out.dimensions(), (160, 60));
        // Covered area is opaque and follows the gradient at every sampled bed x.
        for bx in [15.0, 30.0, 45.0, 60.0, 75.0] {
            let px = ((bx - view.x) * 2.0) as u32;
            let p = out.get_pixel(px, 30);
            assert_eq!(p.0[3], 255, "x={bx}");
            assert!((p.0[0] as f64 - bx * 2.0).abs() <= 2.0, "x={bx} got {}", p.0[0]);
        }
        // Far corner not covered by either capture.
        assert_eq!(out.get_pixel(159, 0).0[3], 0);
    }

    #[test]
    fn exposure_and_brightness() {
        let dark = RgbaImage::from_pixel(4, 4, Rgba([30, 30, 30, 255]));
        let m = mean_luma(&dark);
        assert!((m - 30.0 / 255.0).abs() < 0.01);
        let e = suggest_exposure(100.0, m, 0.45, (1.0, 10_000.0));
        assert!(e > 100.0);
        assert_eq!(suggest_exposure(100.0, 0.9, 0.45, (80.0, 200.0)).round(), 80.0f64.max(100.0 * 0.5f64.powf(0.8)).round());
        let bright = adjust_brightness_contrast(&dark, 0.2, 0.0);
        assert!(mean_luma(&bright) > m + 0.15);
        let same = adjust_brightness_contrast(&dark, 0.0, 0.0);
        assert_eq!(same, dark);
        let h = luma_histogram(&dark);
        assert_eq!(h[30], 16);
        assert_eq!(to_grayscale(&dark).get_pixel(0, 0)[0], 30);
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let mut s = CaptureSettings { camera_name: "USB Cam".into(), exposure: Some(-5.0), ..Default::default() };
        s.lens = Some(LensModel::ideal(640, 480, 500.0));
        s.alignment = Some(CameraAlignment::solve(&targets(4), ImageOrientation::default(), (1280, 720)).unwrap());
        s.kind = CameraKind::HeadMounted;
        let back = CaptureSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        // Minimal file loads with defaults.
        let min = CaptureSettings::from_json(r#"{"version":1,"camera_name":"x"}"#).unwrap();
        assert_eq!(min.kind, CameraKind::Overhead);
        assert!(min.state.show);
        assert_eq!(min.focal_scale, 1.0);
        assert!(CaptureSettings::from_json("nope").is_err());
    }

    #[test]
    fn trace_applies_lens_and_alignment() {
        let lens = LensModel { k: [-0.2, 0.0, 0.0, 0.0], ..LensModel::ideal(200, 120, 150.0) };
        let raw = RgbaImage::from_pixel(200, 120, Rgba([10, 10, 10, 255]));
        let t = trace_from_capture(&raw, Some(&lens), 1.0, None);
        assert_eq!(t.image.dimensions(), (200, 120));
        assert!(t.placement.is_none());
        let tg: Vec<AlignmentTarget> = [(0.0, 0.0), (100.0, 0.0), (100.0, 60.0), (0.0, 60.0)]
            .iter()
            .map(|&(bx, by)| AlignmentTarget { image: Point::new(bx * 2.0, 120.0 - by * 2.0), bed: Point::new(bx, by) })
            .collect();
        let al = CameraAlignment::solve(&tg, ImageOrientation::default(), (200, 120)).unwrap();
        let view = BedRect::new(0.0, 0.0, 100.0, 60.0);
        let t = trace_from_capture(&raw, None, 1.0, Some((&al, view, 2.0)));
        assert_eq!(t.image.dimensions(), (200, 120));
        assert_eq!(t.placement, Some((view, 2.0)));
        assert_eq!(t.image.get_pixel(100, 60)[0], 10);
    }

    #[test]
    fn default_targets_layout() {
        let b = BedRect::new(0.0, 0.0, 400.0, 300.0);
        let t = default_targets(b, 20.0, 4);
        assert_eq!(t, vec![Point::new(20.0, 20.0), Point::new(380.0, 20.0), Point::new(380.0, 280.0), Point::new(20.0, 280.0)]);
        assert_eq!(default_targets(b, 20.0, 9).len(), 9);
    }
}
