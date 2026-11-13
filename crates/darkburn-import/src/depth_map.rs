//! Turns a 3D model into a grey height picture for 3D engraving.
//!
//! The model is looked at straight on (orthographic) from the top, the front or the side,
//! optionally from the opposite side. A z-buffer over its triangles keeps the nearest surface at
//! every pixel centre, and the depth below the nearest visible point becomes a grey tone: by
//! default near is white and the far end of the depth range (and every pixel the model does not
//! cover) is black.

use std::io::Cursor;

use crate::ImportError;
use crate::mesh::{Mesh, Vertex};

/// Which way the model is looked at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    /// From above, looking down Z: picture right is +X, up is +Y, near is high Z.
    #[default]
    Top,
    /// From the front, looking along +Y: right is +X, up is +Z, near is low Y.
    Front,
    /// From the right, looking along -X: right is +Y, up is +Z, near is high X.
    Side,
}

impl View {
    pub const ALL: [View; 3] = [View::Top, View::Front, View::Side];

    /// The name used in commands and settings.
    pub fn name(self) -> &'static str {
        match self {
            View::Top => "top",
            View::Front => "front",
            View::Side => "side",
        }
    }

    /// The view with this name, any case.
    pub fn from_name(name: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.name().eq_ignore_ascii_case(name))
    }

    /// Picture right, picture up and nearness of a model point. `flip` looks from the opposite
    /// side: nearness turns round and the picture is mirrored left to right.
    pub fn project(self, flip: bool, p: Vertex) -> [f64; 3] {
        let [x, y, z] = p.map(f64::from);
        let [u, v, d] = match self {
            View::Top => [x, y, z],
            View::Front => [x, z, -y],
            View::Side => [y, z, x],
        };
        if flip { [-u, v, -d] } else { [u, v, d] }
    }
}

/// How the depth picture is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthSettings {
    pub view: View,
    /// Look from the opposite side (bottom, back or left).
    pub flip: bool,
    /// Width of the picture in millimetres; the height follows the model.
    pub width_mm: f64,
    /// Pixels per inch.
    pub dpi: f64,
    /// Near surfaces white (true) or black (false).
    pub near_white: bool,
    /// Depth below the nearest visible point, in millimetres of the scaled model, that gets the
    /// near tone. Anything nearer is clamped to it. `None` = 0.
    pub depth_from: Option<f64>,
    /// Depth below the nearest visible point that gets the far tone. Anything deeper is clamped
    /// to it. `None` = the farthest visible point.
    pub depth_to: Option<f64>,
}

impl Default for DepthSettings {
    fn default() -> Self {
        DepthSettings { view: View::Top, flip: false, width_mm: 50.0, dpi: 254.0, near_white: true, depth_from: None, depth_to: None }
    }
}

/// How big a depth picture and its work may get.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepthLimits {
    pub max_pixels: u64,
    pub max_side: u32,
    /// The most pixel tests over all triangles (the sum of their bounding-box areas in pixels).
    pub max_work: u64,
}

impl Default for DepthLimits {
    fn default() -> Self {
        DepthLimits { max_pixels: 25_000_000, max_side: 20_000, max_work: 2_000_000_000 }
    }
}

/// Picture size in pixels and millimetres for a model and settings, checked against the limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub px_width: u32,
    pub px_height: u32,
    pub width_mm: f64,
    pub height_mm: f64,
    /// Millimetres per model unit.
    pub scale: f64,
    /// Model extent along the view direction (in model units).
    pub model_depth: f64,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// Works out the picture size without drawing anything.
pub fn frame(mesh: &Mesh, s: &DepthSettings, limits: &DepthLimits) -> Result<Frame, ImportError> {
    let bad = |m: &str| Err(ImportError::Unsupported(m.into()));
    if !(s.width_mm.is_finite() && s.width_mm > 0.0) {
        return bad("the width must be more than zero");
    }
    if !(s.dpi.is_finite() && s.dpi > 0.0) {
        return bad("the resolution must be more than zero");
    }
    for v in [s.depth_from, s.depth_to].into_iter().flatten() {
        if !(v.is_finite() && v >= 0.0) {
            return bad("the depth range must be zero or more");
        }
    }
    if let (Some(a), Some(b)) = (s.depth_from, s.depth_to)
        && b <= a
    {
        return bad("the depth range must end deeper than it starts");
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for &p in mesh.triangles.iter().flatten() {
        let q = s.view.project(s.flip, p);
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    if mesh.triangles.is_empty() || !(w.is_finite() && w > 0.0) || !h.is_finite() {
        return bad("the model has no width seen from this side");
    }
    let scale = s.width_mm / w;
    let height_mm = h * scale;
    let px = |mm: f64| (mm / 25.4 * s.dpi).round().max(1.0);
    let (pw, ph) = (px(s.width_mm), px(height_mm));
    if pw > f64::from(limits.max_side) || ph > f64::from(limits.max_side) || pw * ph > limits.max_pixels as f64 {
        return Err(ImportError::Unsupported(format!(
            "the picture would be {pw} x {ph} pixels, more than {} million; lower the width or the resolution",
            limits.max_pixels / 1_000_000
        )));
    }
    Ok(Frame { px_width: pw as u32, px_height: ph as u32, width_mm: s.width_mm, height_mm, scale, model_depth: hi[2] - lo[2], lo, hi })
}

/// The z-buffer: the nearness of the nearest surface at every pixel centre, in millimetres of the
/// scaled model, row by row from the top. Pixels the model does not cover hold `NEG_INFINITY`.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthBuffer {
    pub frame: Frame,
    pub near: Vec<f32>,
}

impl DepthBuffer {
    /// Nearness at a pixel, `None` where the model does not cover it.
    pub fn at(&self, x: u32, y: u32) -> Option<f32> {
        let v = *self.near.get(y as usize * self.frame.px_width as usize + x as usize)?;
        v.is_finite().then_some(v)
    }

    /// The nearest and the farthest covered nearness, `None` when nothing is covered.
    pub fn visible_range(&self) -> Option<(f32, f32)> {
        let mut r: Option<(f32, f32)> = None;
        for &v in self.near.iter().filter(|v| v.is_finite()) {
            r = Some(r.map_or((v, v), |(near, far)| (near.max(v), far.min(v))));
        }
        r
    }

    /// The visible depth of the model (nearest to farthest covered point) in millimetres.
    pub fn visible_depth(&self) -> f64 {
        self.visible_range().map_or(0.0, |(near, far)| f64::from(near - far))
    }

    /// The grey tones, 0 to 65535, one per pixel.
    pub fn tones(&self, s: &DepthSettings) -> Vec<u16> {
        let (near, far) = self.visible_range().unwrap_or((0.0, 0.0));
        let from = s.depth_from.unwrap_or(0.0);
        let to = s.depth_to.unwrap_or(f64::from(near - far));
        let tone = |t: f64| {
            let t = t.clamp(0.0, 1.0);
            let level = if s.near_white { 1.0 - t } else { t };
            (level * 65535.0).round() as u16
        };
        let far_tone = tone(1.0);
        self.near
            .iter()
            .map(|&v| {
                if !v.is_finite() {
                    return far_tone;
                }
                let depth = f64::from(near - v);
                if to > from { tone((depth - from) / (to - from)) } else { tone(0.0) }
            })
            .collect()
    }
}

/// Draws the z-buffer of a model.
pub fn depth_buffer(mesh: &Mesh, s: &DepthSettings, limits: &DepthLimits) -> Result<DepthBuffer, ImportError> {
    let f = frame(mesh, s, limits)?;
    let (w, h) = (f.px_width as usize, f.px_height as usize);
    let (span_u, span_v) = (f.hi[0] - f.lo[0], f.hi[1] - f.lo[1]);
    let to_px = |q: [f64; 3]| {
        let x = (q[0] - f.lo[0]) / span_u * w as f64;
        let y = if span_v > 0.0 { (f.hi[1] - q[1]) / span_v * h as f64 } else { h as f64 * 0.5 };
        // Nearness in millimetres, measured from the far side of the model's box.
        [x, y, (q[2] - f.lo[2]) * f.scale]
    };
    let mut near = vec![f32::NEG_INFINITY; w * h];
    let mut work: u64 = 0;
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|p| to_px(s.view.project(s.flip, p)));
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() <= 1e-12 {
            continue;
        }
        // Pixels whose centre (i + 0.5) can lie inside the triangle.
        let span = |lo: f64, hi: f64, n: usize| {
            let first = (lo - 0.5).ceil().max(0.0);
            let last = (hi - 0.5).floor().min(n as f64 - 1.0);
            (first <= last).then_some((first as usize, last as usize))
        };
        let (Some((x0, x1)), Some((y0, y1))) =
            (span(a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]), w), span(a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]), h))
        else {
            continue;
        };
        work += ((x1 - x0 + 1) * (y1 - y0 + 1)) as u64;
        if work > limits.max_work {
            return Err(ImportError::Unsupported("the model is too detailed for this picture size; lower the resolution".into()));
        }
        let edge = |p: [f64; 3], q: [f64; 3], x: f64, y: f64| (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0]);
        for y in y0..=y1 {
            let cy = y as f64 + 0.5;
            let row = &mut near[y * w..(y + 1) * w];
            for (x, slot) in row.iter_mut().enumerate().take(x1 + 1).skip(x0) {
                let cx = x as f64 + 0.5;
                let (wa, wb, wc) = (edge(b, c, cx, cy) / area, edge(c, a, cx, cy) / area, edge(a, b, cx, cy) / area);
                // Either winding: dividing by the signed area makes inside positive.
                if wa < -1e-9 || wb < -1e-9 || wc < -1e-9 {
                    continue;
                }
                let d = (wa * a[2] + wb * b[2] + wc * c[2]) as f32;
                if d > *slot {
                    *slot = d;
                }
            }
        }
    }
    Ok(DepthBuffer { frame: f, near })
}

/// A finished depth picture: 16-bit grey tones, row by row from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub frame: Frame,
    pub tones: Vec<u16>,
    /// The visible depth of the scaled model in millimetres.
    pub visible_depth_mm: f64,
}

impl DepthMap {
    /// The picture as a 16-bit grey PNG file.
    pub fn to_png(&self) -> Result<Vec<u8>, ImportError> {
        let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(self.frame.px_width, self.frame.px_height, self.tones.clone())
            .ok_or_else(|| ImportError::Parse("the depth picture has the wrong size".into()))?;
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma16(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| ImportError::Parse(format!("could not encode the depth picture: {e}")))?;
        Ok(out.into_inner())
    }
}

/// Makes the depth picture of a model.
pub fn depth_map(mesh: &Mesh, s: &DepthSettings, limits: &DepthLimits) -> Result<DepthMap, ImportError> {
    let buffer = depth_buffer(mesh, s, limits)?;
    Ok(DepthMap { frame: buffer.frame, tones: buffer.tones(s), visible_depth_mm: buffer.visible_depth() })
}
