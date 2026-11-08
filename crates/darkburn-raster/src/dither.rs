//! Dithering and screening. All functions take a *tone* buffer, `0` = no burn, `1` = full
//! burn, and return `0` / `255` per pixel.
//!
//! The error-diffusion weights are the published ones (Floyd & Steinberg 1976; Jarvis, Judice
//! & Ninke 1976; Stucki 1981; Atkinson's MacPaint dither). The code is written from scratch.

use serde::{Deserialize, Serialize};

/// An error-diffusion kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKernel {
    FloydSteinberg,
    JarvisJudiceNinke,
    Stucki,
    /// Spreads only 3/4 of the error, which keeps highlights and shadows clean.
    Atkinson,
}

impl ErrorKernel {
    /// `(dx, dy, weight)` entries and the divisor.
    fn weights(self) -> (&'static [(i32, i32, f32)], f32) {
        match self {
            Self::FloydSteinberg => (&[(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)], 16.0),
            Self::JarvisJudiceNinke => (
                &[
                    (1, 0, 7.0),
                    (2, 0, 5.0),
                    (-2, 1, 3.0),
                    (-1, 1, 5.0),
                    (0, 1, 7.0),
                    (1, 1, 5.0),
                    (2, 1, 3.0),
                    (-2, 2, 1.0),
                    (-1, 2, 3.0),
                    (0, 2, 5.0),
                    (1, 2, 3.0),
                    (2, 2, 1.0),
                ],
                48.0,
            ),
            Self::Stucki => (
                &[
                    (1, 0, 8.0),
                    (2, 0, 4.0),
                    (-2, 1, 2.0),
                    (-1, 1, 4.0),
                    (0, 1, 8.0),
                    (1, 1, 4.0),
                    (2, 1, 2.0),
                    (-2, 2, 1.0),
                    (-1, 2, 2.0),
                    (0, 2, 4.0),
                    (1, 2, 2.0),
                    (2, 2, 1.0),
                ],
                42.0,
            ),
            Self::Atkinson => (&[(1, 0, 1.0), (2, 0, 1.0), (-1, 1, 1.0), (0, 1, 1.0), (1, 1, 1.0), (0, 2, 1.0)], 8.0),
        }
    }
}

const PAD: usize = 2;

/// Error diffusion. With `serpentine` every other row runs right to left, which avoids the
/// diagonal "worm" artefacts of plain raster order.
pub fn error_diffuse(tone: &[f32], width: u32, height: u32, kernel: ErrorKernel, serpentine: bool) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(tone.len(), w * h);
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    let (entries, div) = kernel.weights();
    // Pre-scaled weights split by row so the inner loop has no branches on dy.
    let row0: Vec<(i32, f32)> = entries.iter().filter(|e| e.1 == 0).map(|e| (e.0, e.2 / div)).collect();
    let row1: Vec<(i32, f32)> = entries.iter().filter(|e| e.1 == 1).map(|e| (e.0, e.2 / div)).collect();
    let row2: Vec<(i32, f32)> = entries.iter().filter(|e| e.1 == 2).map(|e| (e.0, e.2 / div)).collect();
    let stride = w + 2 * PAD;
    let mut e0 = vec![0.0f32; stride];
    let mut e1 = vec![0.0f32; stride];
    let mut e2 = vec![0.0f32; stride];
    for y in 0..h {
        let reverse = serpentine && y % 2 == 1;
        let src = &tone[y * w..(y + 1) * w];
        let dst = &mut out[y * w..(y + 1) * w];
        for i in 0..w {
            let x = if reverse { w - 1 - i } else { i };
            let t = src[x];
            let v = t + e0[x + PAD];
            let on = v >= 0.5;
            // Pure white and pure black never spread error, so their edges stay clean.
            let err = if t <= 0.0 || t >= 1.0 { 0.0 } else { v - if on { 1.0 } else { 0.0 } };
            if on {
                dst[x] = 255;
            }
            if err != 0.0 {
                let xi = (x + PAD) as i32;
                for &(dx, wt) in &row0 {
                    let tx = if reverse { xi - dx } else { xi + dx };
                    e0[tx as usize] += err * wt;
                }
                for &(dx, wt) in &row1 {
                    let tx = if reverse { xi - dx } else { xi + dx };
                    e1[tx as usize] += err * wt;
                }
                for &(dx, wt) in &row2 {
                    let tx = if reverse { xi - dx } else { xi + dx };
                    e2[tx as usize] += err * wt;
                }
            }
        }
        std::mem::swap(&mut e0, &mut e1);
        std::mem::swap(&mut e1, &mut e2);
        e2.iter_mut().for_each(|v| *v = 0.0);
    }
    out
}

/// An `n × n` Bayer threshold matrix (`n` a power of two), values in `(0, 1)`, row-major.
pub fn bayer_matrix(n: usize) -> Vec<f32> {
    assert!(n.is_power_of_two());
    let mut m = vec![0u32];
    let mut size = 1;
    while size < n {
        let s2 = size * 2;
        let mut next = vec![0u32; s2 * s2];
        for y in 0..size {
            for x in 0..size {
                let v = 4 * m[y * size + x];
                next[y * s2 + x] = v;
                next[y * s2 + x + size] = v + 2;
                next[(y + size) * s2 + x] = v + 3;
                next[(y + size) * s2 + x + size] = v + 1;
            }
        }
        m = next;
        size = s2;
    }
    let total = (n * n) as f32;
    m.into_iter().map(|v| (v as f32 + 0.5) / total).collect()
}

/// Ordered dither against a threshold tile. `gx0, gy0` is the global lattice position of the
/// top-left pixel, so neighbouring images share one pattern.
pub fn screen(tone: &[f32], width: u32, height: u32, tile: &[f32], tile_n: usize, gx0: i64, gy0: i64) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let n = tile_n as i64;
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let ty = ((gy0 + y as i64).rem_euclid(n)) as usize;
        let trow = &tile[ty * tile_n..(ty + 1) * tile_n];
        let mut tx = (gx0.rem_euclid(n)) as usize;
        for (o, &t) in out[y * w..(y + 1) * w].iter_mut().zip(&tone[y * w..(y + 1) * w]) {
            if t > trow[tx] {
                *o = 255;
            }
            tx += 1;
            if tx == tile_n {
                tx = 0;
            }
        }
    }
    out
}

/// The 45° clustered-dot newsprint screen: an 8 × 8 tile holding two dots, cell pitch
/// `4√2 ≈ 5.7` pixels. Thresholds rank pixels by distance to the nearest dot centre.
pub fn newsprint_tile() -> Vec<f32> {
    const N: usize = 8;
    let centres = [(0.0, 0.0), (8.0, 0.0), (0.0, 8.0), (8.0, 8.0), (4.0, 4.0)];
    let mut order: Vec<(f32, usize)> = (0..N * N)
        .map(|i| {
            let (x, y) = ((i % N) as f32 + 0.5, (i / N) as f32 + 0.5);
            let d = centres.iter().map(|&(cx, cy)| (x - cx).hypot(y - cy)).fold(f32::MAX, f32::min);
            (d, i)
        })
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut tile = vec![0.0; N * N];
    for (rank, &(_, i)) in order.iter().enumerate() {
        tile[i] = (rank as f32 + 0.5) / (N * N) as f32;
    }
    tile
}

/// Amplitude-modulated halftone: round dots on a grid of `cell_px` pixel cells rotated by
/// `angle_deg`. Each dot's area matches the mean tone of its cell, so tone is reproduced
/// accurately; above 78.5 % coverage the dots merge and the white gaps shrink instead.
pub fn halftone(tone: &[f32], width: u32, height: u32, cell_px: f64, angle_deg: f64, gx0: i64, gy0: i64) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h];
    if w == 0 || h == 0 {
        return out;
    }
    let cell = cell_px.max(1.0);
    // Integral image for cell averages.
    let iw = w + 1;
    let mut integral = vec![0.0f64; iw * (h + 1)];
    for y in 0..h {
        let mut run = 0.0;
        for x in 0..w {
            run += f64::from(tone[y * w + x]);
            integral[(y + 1) * iw + x + 1] = integral[y * iw + x + 1] + run;
        }
    }
    let box_mean = |cx: f64, cy: f64| -> f64 {
        let half = cell / 2.0;
        let x0 = ((cx - half).round().max(0.0) as usize).min(w);
        let x1 = ((cx + half).round().max(0.0) as usize).min(w);
        let y0 = ((cy - half).round().max(0.0) as usize).min(h);
        let y1 = ((cy + half).round().max(0.0) as usize).min(h);
        if x1 <= x0 || y1 <= y0 {
            let xi = (cx.max(0.0) as usize).min(w - 1);
            let yi = (cy.max(0.0) as usize).min(h - 1);
            return f64::from(tone[yi * w + xi]);
        }
        let s = integral[y1 * iw + x1] - integral[y0 * iw + x1] - integral[y1 * iw + x0] + integral[y0 * iw + x0];
        s / ((x1 - x0) * (y1 - y0)) as f64
    };
    let (sin, cos) = angle_deg.to_radians().sin_cos();
    for y in 0..h {
        for x in 0..w {
            let t = tone[y * w + x];
            if t <= 0.0 {
                continue;
            }
            // Global lattice position of this pixel centre, rotated into the screen frame.
            let gx = (gx0 + x as i64) as f64 + 0.5;
            let gy = (gy0 + y as i64) as f64 + 0.5;
            let u = (cos * gx + sin * gy) / cell;
            let v = (-sin * gx + cos * gy) / cell;
            let (iu, iv) = (u.floor(), v.floor());
            let (fu, fv) = (u - iu - 0.5, v - iv - 0.5);
            // Cell centre back in local pixel coordinates.
            let (cu, cv) = ((iu + 0.5) * cell, (iv + 0.5) * cell);
            let cx = cos * cu - sin * cv - gx0 as f64;
            let cy = sin * cu + cos * cv - gy0 as f64;
            let d = box_mean(cx, cy).clamp(0.0, 1.0);
            let on = if d <= std::f64::consts::FRAC_PI_4 {
                fu.hypot(fv) < (d / std::f64::consts::PI).sqrt()
            } else {
                (0.5 - fu.abs()).hypot(0.5 - fv.abs()) >= ((1.0 - d) / std::f64::consts::PI).sqrt()
            };
            if on {
                out[y * w + x] = 255;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(out: &[u8]) -> f32 {
        out.iter().filter(|&&v| v != 0).count() as f32 / out.len() as f32
    }

    #[test]
    fn diffusion_preserves_mean_tone() {
        for k in [ErrorKernel::FloydSteinberg, ErrorKernel::JarvisJudiceNinke, ErrorKernel::Stucki] {
            for t in [0.1f32, 0.25, 0.5, 0.8] {
                let tone = vec![t; 64 * 64];
                let c = coverage(&error_diffuse(&tone, 64, 64, k, true));
                assert!((c - t).abs() < 0.02, "{k:?} {t} -> {c}");
            }
        }
        // Atkinson drops a quarter of the error so mid-tones are close but not exact.
        let c = coverage(&error_diffuse(&vec![0.5; 64 * 64], 64, 64, ErrorKernel::Atkinson, false));
        assert!((c - 0.5).abs() < 0.05);
    }

    #[test]
    fn pure_tones_are_exact() {
        for k in [ErrorKernel::FloydSteinberg, ErrorKernel::Atkinson] {
            assert_eq!(coverage(&error_diffuse(&[0.0; 100], 10, 10, k, true)), 0.0);
            assert_eq!(coverage(&error_diffuse(&[1.0; 100], 10, 10, k, true)), 1.0);
        }
    }

    #[test]
    fn bayer_is_a_permutation() {
        let m = bayer_matrix(8);
        let mut ranks: Vec<u32> = m.iter().map(|v| (v * 64.0 - 0.5).round() as u32).collect();
        ranks.sort();
        assert_eq!(ranks, (0..64).collect::<Vec<_>>());
        assert_eq!(bayer_matrix(2), vec![0.125, 0.625, 0.875, 0.375]);
    }

    #[test]
    fn ordered_and_newsprint_cover_tone() {
        let bayer = bayer_matrix(8);
        let news = newsprint_tile();
        for t in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let tone = vec![t; 32 * 32];
            assert!((coverage(&screen(&tone, 32, 32, &bayer, 8, 0, 0)) - t).abs() < 0.02);
            assert!((coverage(&screen(&tone, 32, 32, &news, 8, 3, -5)) - t).abs() < 0.02);
        }
    }

    #[test]
    fn screen_is_anchored_to_lattice() {
        let bayer = bayer_matrix(8);
        let tone = vec![0.4; 16 * 16];
        let a = screen(&tone, 16, 16, &bayer, 8, 0, 0);
        let b = screen(&tone[16 * 8..], 16, 8, &bayer, 8, 0, 8);
        assert_eq!(&a[16 * 8..], &b[..]);
    }

    #[test]
    fn halftone_tone_accuracy() {
        for t in [0.1f32, 0.3, 0.5, 0.9] {
            let tone = vec![t; 120 * 120];
            for angle in [0.0, 45.0, 22.5] {
                let c = coverage(&halftone(&tone, 120, 120, 10.0, angle, 0, 0));
                assert!((c - t).abs() < 0.04, "{t} {angle} -> {c}");
            }
        }
        assert_eq!(coverage(&halftone(&[0.0; 100], 10, 10, 4.0, 0.0, 0, 0)), 0.0);
    }
}
