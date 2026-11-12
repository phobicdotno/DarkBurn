//! Scan grids and resampling an image onto one.
//!
//! A scan grid is a lattice of square cells, `interval_mm` on a side, in the *scan frame*: the
//! machine frame rotated by the scan angle. Lattice cell `(c, r)` covers
//! `[c·i, (c+1)·i) × [r·i, (r+1)·i)` in the scan frame, and scanline `r` runs along
//! `y = (r + 0.5)·i`. The lattice is anchored at the machine origin, so every image on a layer
//! with the same angle and interval lands on the same lines and can be merged into one scan.

use kurbo::{Affine, Point, Vec2};

use crate::gray::{GrayF, fast_to_linear, fast_to_srgb, snap_endpoints};

/// A rectangular window of the global scan lattice. Grid pixel `(x, y)` (rows top to bottom)
/// is lattice cell `(col0 + x, row_top - y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanGrid {
    pub interval_mm: f64,
    pub angle_deg: f64,
    pub col0: i64,
    pub row_top: i64,
    pub width: u32,
    pub height: u32,
}

/// How source pixels are sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    /// Area average in linear light (photos).
    Area,
    /// Nearest pixel (already-dithered images, pass-through).
    Nearest,
}

impl ScanGrid {
    /// Scan frame to machine millimetres (a rotation by the scan angle).
    pub fn scan_to_mm(&self) -> Affine {
        Affine::rotate(self.angle_deg.to_radians())
    }

    /// Grid pixel coordinates (x right, y down, pixel `(0,0)` covering `[0,1)²`) to machine mm.
    pub fn pixel_to_mm(&self) -> Affine {
        let i = self.interval_mm;
        self.scan_to_mm() * Affine::new([i, 0.0, 0.0, -i, self.col0 as f64 * i, (self.row_top + 1) as f64 * i])
    }

    /// Lattice row of grid row `y`.
    pub fn lattice_row(&self, y: u32) -> i64 {
        self.row_top - i64::from(y)
    }

    /// Scan-frame Y (mm) of the scanline through grid row `y`.
    pub fn line_y(&self, y: u32) -> f64 {
        (self.lattice_row(y) as f64 + 0.5) * self.interval_mm
    }

    /// The smallest grid covering an image of `width × height` pixels placed by `image_to_mm`.
    pub fn for_image(width: u32, height: u32, image_to_mm: Affine, interval_mm: f64, angle_deg: f64) -> Self {
        let i = interval_mm.max(1e-6);
        let to_scan = Affine::rotate(-angle_deg.to_radians()) * image_to_mm;
        let corners = [(0.0, 0.0), (f64::from(width), 0.0), (0.0, f64::from(height)), (f64::from(width), f64::from(height))]
            .map(|(x, y)| to_scan * Point::new(x, y));
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in corners {
            x0 = x0.min(p.x);
            x1 = x1.max(p.x);
            y0 = y0.min(p.y);
            y1 = y1.max(p.y);
        }
        // A tiny tolerance keeps exact edges from adding an empty row or column.
        let eps = 1e-7;
        let col0 = (x0 / i + eps).floor() as i64;
        // A position far outside any machine pins the lattice numbers at the ends of their
        // range: the arithmetic below must not run over there.
        let col1 = ((x1 / i - eps).ceil() as i64).max(col0.saturating_add(1));
        let row0 = (y0 / i + eps).floor() as i64;
        let row1 = ((y1 / i - eps).ceil() as i64).max(row0.saturating_add(1));
        let count = |from: i64, to: i64| u32::try_from(to.saturating_sub(from)).unwrap_or(u32::MAX);
        Self { interval_mm: i, angle_deg, col0, row_top: row1.saturating_sub(1), width: count(col0, col1), height: count(row0, row1) }
    }
}

/// Resamples `src` (placed by `image_to_mm`) onto `grid`.
///
/// Returns the sRGB-encoded grey values and, per grid pixel, the fraction covered by the
/// image (`0` outside it and where it is fully transparent, so that no adjustment or negative
/// can make those places burn). Area resampling averages in linear light with up to 8×8 samples per
/// cell, so downscaling keeps the true tone; values within half a step of black or white snap
/// to it.
pub fn resample_to_grid(src: &GrayF, image_to_mm: Affine, grid: &ScanGrid, mode: Resample) -> (GrayF, Vec<f32>) {
    let (gw, gh) = (grid.width as usize, grid.height as usize);
    let mut out = GrayF::new(grid.width, grid.height, 1.0);
    let mut coverage = vec![0.0f32; gw * gh];
    if src.width == 0 || src.height == 0 || gw == 0 || gh == 0 {
        return (out, coverage);
    }
    let m = image_to_mm.inverse() * grid.pixel_to_mm();
    let c = m.as_coeffs();
    let ax = Vec2::new(c[0], c[1]); // source step per grid x
    let ay = Vec2::new(c[2], c[3]); // source step per grid y
    let (sw, sh) = (src.width as f64, src.height as f64);
    match mode {
        Resample::Nearest => {
            for gy in 0..gh {
                for gx in 0..gw {
                    let p = m * Point::new(gx as f64 + 0.5, gy as f64 + 0.5);
                    if p.x >= 0.0 && p.y >= 0.0 && p.x < sw && p.y < sh && !src.is_clear(p.x as u32, p.y as u32) {
                        let k = gy * gw + gx;
                        out.data[k] = src.get(p.x as u32, p.y as u32);
                        coverage[k] = 1.0;
                    }
                }
            }
        }
        Resample::Area => {
            let lin = src.to_linear();
            let nx = (ax.hypot().ceil() as usize).clamp(1, 8);
            let ny = (ay.hypot().ceil() as usize).clamp(1, 8);
            let (w, h) = (src.width as usize, src.height as usize);
            let sample = |x: f64, y: f64| -> Option<f32> {
                // A fully transparent pixel is as much outside the image as the area around it.
                if x < 0.0 || y < 0.0 || x >= sw || y >= sh || src.is_clear(x as u32, y as u32) {
                    return None;
                }
                // Bilinear between pixel centres, clamped at the edges.
                let fx = (x - 0.5).max(0.0);
                let fy = (y - 0.5).max(0.0);
                let x0 = (fx as usize).min(w - 1);
                let y0 = (fy as usize).min(h - 1);
                let x1 = (x0 + 1).min(w - 1);
                let y1 = (y0 + 1).min(h - 1);
                let tx = (fx - x0 as f64).min(1.0) as f32;
                let ty = (fy - y0 as f64).min(1.0) as f32;
                // A transparent neighbour has no tone to blend with: the pixel's own is used.
                let own = lin[y as usize * w + x as usize];
                let tap = |px: usize, py: usize| if src.clear.is_empty() || !src.clear[py * w + px] { lin[py * w + px] } else { own };
                let a = tap(x0, y0) + (tap(x1, y0) - tap(x0, y0)) * tx;
                let b = tap(x0, y1) + (tap(x1, y1) - tap(x0, y1)) * tx;
                Some(a + (b - a) * ty)
            };
            let inv_n = 1.0 / (nx * ny) as f32;
            for gy in 0..gh {
                for gx in 0..gw {
                    let mut sum = 0.0f32;
                    let mut hits = 0u32;
                    for j in 0..ny {
                        for i in 0..nx {
                            let p = m * Point::new(gx as f64 + (i as f64 + 0.5) / nx as f64, gy as f64 + (j as f64 + 0.5) / ny as f64);
                            if let Some(v) = sample(p.x, p.y) {
                                sum += v;
                                hits += 1;
                            }
                        }
                    }
                    if hits > 0 {
                        let k = gy * gw + gx;
                        out.data[k] = snap_endpoints(fast_to_srgb(sum / hits as f32));
                        coverage[k] = hits as f32 * inv_n;
                    }
                }
            }
        }
    }
    (out, coverage)
}

/// Composites `img` over white using `coverage` in linear light (outside the image is white).
pub(crate) fn composite_over_white(img: &mut GrayF, coverage: &[f32]) {
    for (v, &c) in img.data.iter_mut().zip(coverage) {
        if c >= 1.0 {
            continue;
        }
        if c <= 0.0 {
            *v = 1.0;
        } else {
            *v = snap_endpoints(fast_to_srgb(fast_to_linear(*v) * c + (1.0 - c)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement;
    use kurbo::Size;

    #[test]
    fn a_picture_placed_absurdly_far_away_still_gets_a_grid() {
        // Found by the monkey (seed 1025): a picture at x = 5e299 mm overflowed the column count.
        for far in [5e299, -5e299] {
            let t = placement(320, 160, Point::new(far, far), Size::new(32.0, 16.0));
            let g = ScanGrid::for_image(320, 160, t, 0.1, 0.0);
            let src = GrayF::new(320, 160, 0.5);
            let (out, coverage) = resample_to_grid(&src, t, &g, Resample::Area);
            assert_eq!(coverage.len(), (g.width as usize) * (g.height as usize));
            assert_eq!(out.data.len(), coverage.len());
        }
    }

    #[test]
    fn grid_matches_image_exactly_when_aligned() {
        // 100 × 50 px image, 10 × 5 mm at (2, 3), 0.1 mm interval: one cell per pixel.
        let t = placement(100, 50, Point::new(2.0, 3.0), Size::new(10.0, 5.0));
        let g = ScanGrid::for_image(100, 50, t, 0.1, 0.0);
        assert_eq!((g.width, g.height, g.col0, g.row_top), (100, 50, 20, 79));
        let p = g.pixel_to_mm() * Point::new(0.0, 0.0);
        assert!((p.x - 2.0).abs() < 1e-9 && (p.y - 8.0).abs() < 1e-9);
        assert!((g.line_y(49) - 3.05).abs() < 1e-9);
    }

    #[test]
    fn identity_resample_is_exact() {
        let src = GrayF::from_fn(16, 8, |x, y| ((x + y) % 5) as f32 / 4.0);
        let t = placement(16, 8, Point::new(0.0, 0.0), Size::new(1.6, 0.8));
        let g = ScanGrid::for_image(16, 8, t, 0.1, 0.0);
        let (out, cov) = resample_to_grid(&src, t, &g, Resample::Area);
        assert!(cov.iter().all(|&c| c == 1.0));
        for (a, b) in out.data.iter().zip(&src.data) {
            assert!((a - b).abs() < 3e-3, "{a} {b}");
        }
        let (near, _) = resample_to_grid(&src, t, &g, Resample::Nearest);
        assert_eq!(near, src);
    }

    #[test]
    fn downscale_averages_in_linear_light() {
        // A 1-px black/white checkerboard averages to 50 % linear light, about 0.735 sRGB,
        // not 0.5.
        let src = GrayF::from_fn(64, 64, |x, y| ((x + y) % 2) as f32);
        let t = placement(64, 64, Point::ZERO, Size::new(6.4, 6.4));
        let g = ScanGrid::for_image(64, 64, t, 0.4, 0.0);
        assert_eq!(g.width, 16);
        let (out, _) = resample_to_grid(&src, t, &g, Resample::Area);
        for v in out.data {
            assert!((v - 0.7354).abs() < 0.01, "{v}");
        }
    }

    #[test]
    fn white_and_black_stay_true() {
        let src = GrayF::from_fn(10, 10, |x, _| if x < 5 { 0.0 } else { 1.0 });
        let t = placement(10, 10, Point::ZERO, Size::new(10.0, 10.0));
        let g = ScanGrid::for_image(10, 10, t, 0.25, 0.0);
        let (out, _) = resample_to_grid(&src, t, &g, Resample::Area);
        assert_eq!(out.get(0, 0), 0.0);
        assert_eq!(out.get(g.width - 1, 0), 1.0);
    }

    #[test]
    fn rotated_grid_covers_image() {
        let t = placement(10, 10, Point::ZERO, Size::new(10.0, 10.0));
        let g = ScanGrid::for_image(10, 10, t, 0.5, 45.0);
        // Diagonal of a 10 mm square is ~14.14 mm.
        assert!(g.width >= 28 && g.width <= 30, "{}", g.width);
        let src = GrayF::new(10, 10, 0.0);
        let (_, cov) = resample_to_grid(&src, t, &g, Resample::Area);
        let total: f32 = cov.iter().sum::<f32>() * 0.25;
        assert!((total - 100.0).abs() < 4.0, "{total}");
    }

    #[test]
    fn composite_outside_is_white() {
        let mut g = GrayF::new(3, 1, 0.0);
        composite_over_white(&mut g, &[1.0, 0.0, 0.5]);
        assert_eq!(g.data[0], 0.0);
        assert_eq!(g.data[1], 1.0);
        assert!((g.data[2] - 0.7354).abs() < 0.01);
    }
}
