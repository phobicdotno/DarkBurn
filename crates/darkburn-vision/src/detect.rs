//! Calibration pattern detection: circle grids (threshold + blobs) and checkerboards (saddle points).
//!
//! Both detectors return sub-pixel centres in the pixel convention of this crate (pixel `(i, j)` has its centre at `(i, j)`).

use crate::homography::{Homography, fit_homography_dlt};
use image::GrayImage;
use kurbo::Point;
use serde::{Deserialize, Serialize};

/// What is printed on the calibration target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternKind {
    /// Grid of filled circles. `dark_on_light` true for black dots on white paper.
    CircleGrid { dark_on_light: bool },
    /// Checkerboard; `cols` x `rows` count the inner corners.
    Checkerboard,
}

/// Geometry of the printed target: `cols` x `rows` points `spacing_mm` apart.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternSpec {
    pub kind: PatternKind,
    pub cols: usize,
    pub rows: usize,
    pub spacing_mm: f64,
}

impl PatternSpec {
    pub fn count(&self) -> usize {
        self.cols * self.rows
    }

    /// Plane coordinates (mm) of the grid points in row-major order.
    pub fn object_points(&self) -> Vec<Point> {
        let mut v = Vec::with_capacity(self.count());
        for r in 0..self.rows {
            for c in 0..self.cols {
                v.push(Point::new(c as f64 * self.spacing_mm, r as f64 * self.spacing_mm));
            }
        }
        v
    }
}

/// Detect and order the pattern points (row-major, `cols * rows` of them).
pub fn detect_pattern(img: &GrayImage, spec: &PatternSpec) -> Option<Vec<Point>> {
    if spec.cols < 2 || spec.rows < 2 {
        return None;
    }
    let pts = match spec.kind {
        PatternKind::CircleGrid { dark_on_light } => detect_circle_blobs(img, dark_on_light),
        PatternKind::Checkerboard => {
            let mut c = detect_checkerboard_corners(img);
            let n = spec.count();
            if c.len() < n {
                return None;
            }
            c.truncate(n);
            c.into_iter().map(|(p, _)| p).collect()
        }
    };
    if pts.len() != spec.count() {
        return None;
    }
    order_grid(&pts, spec.cols, spec.rows)
}

/// Otsu threshold of an 8-bit image.
pub fn otsu_threshold(img: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in img.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut wb, mut sum_b, mut best, mut thr) = (0u64, 0.0f64, -1.0f64, 0u8);
    for t in 0..256 {
        wb += hist[t];
        if wb == 0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0 {
            break;
        }
        sum_b += t as f64 * hist[t] as f64;
        let mb = sum_b / wb as f64;
        let mf = (sum_all - sum_b) / wf as f64;
        let between = wb as f64 * wf as f64 * (mb - mf) * (mb - mf);
        if between > best {
            best = between;
            thr = t as u8;
        }
    }
    thr
}

/// Centres of the blobs that look like the dots of a circle grid.
pub fn detect_circle_blobs(img: &GrayImage, dark_on_light: bool) -> Vec<Point> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 8 || h < 8 {
        return Vec::new();
    }
    let thr = otsu_threshold(img);
    let px = |x: usize, y: usize| img.as_raw()[y * w + x];
    let on = |x: usize, y: usize| if dark_on_light { px(x, y) <= thr } else { px(x, y) > thr };

    // Mean levels of the two classes for sub-pixel weighting.
    let (mut sum_on, mut n_on, mut sum_off, mut n_off) = (0.0f64, 0u64, 0.0f64, 0u64);
    for y in 0..h {
        for x in 0..w {
            if on(x, y) {
                sum_on += px(x, y) as f64;
                n_on += 1;
            } else {
                sum_off += px(x, y) as f64;
                n_off += 1;
            }
        }
    }
    if n_on == 0 || n_off == 0 {
        return Vec::new();
    }
    let level_on = sum_on / n_on as f64;
    let level_off = sum_off / n_off as f64;
    let span = (level_off - level_on).abs().max(1.0);

    let mut labels = vec![0u32; w * h];
    struct Blob {
        id: u32,
        area: usize,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
        touches_border: bool,
    }
    let mut blobs: Vec<Blob> = Vec::new();
    let mut stack = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if labels[y * w + x] != 0 || !on(x, y) {
                continue;
            }
            let id = blobs.len() as u32 + 1;
            let mut b = Blob { id, area: 0, x0: x, x1: x, y0: y, y1: y, touches_border: false };
            labels[y * w + x] = id;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                b.area += 1;
                b.x0 = b.x0.min(cx);
                b.x1 = b.x1.max(cx);
                b.y0 = b.y0.min(cy);
                b.y1 = b.y1.max(cy);
                if cx == 0 || cy == 0 || cx == w - 1 || cy == h - 1 {
                    b.touches_border = true;
                }
                let mut push = |nx: usize, ny: usize| {
                    if labels[ny * w + nx] == 0 && on(nx, ny) {
                        labels[ny * w + nx] = id;
                        stack.push((nx, ny));
                    }
                };
                if cx > 0 {
                    push(cx - 1, cy);
                }
                if cx + 1 < w {
                    push(cx + 1, cy);
                }
                if cy > 0 {
                    push(cx, cy - 1);
                }
                if cy + 1 < h {
                    push(cx, cy + 1);
                }
            }
            blobs.push(b);
        }
    }
    let mut areas: Vec<usize> = blobs.iter().filter(|b| b.area >= 12 && !b.touches_border).map(|b| b.area).collect();
    if areas.is_empty() {
        return Vec::new();
    }
    areas.sort_unstable();
    let median = areas[areas.len() / 2] as f64;
    let mut out = Vec::new();
    for b in &blobs {
        if b.touches_border || (b.area as f64) < median * 0.4 || (b.area as f64) > median * 2.5 {
            continue;
        }
        let bw = (b.x1 - b.x0 + 1) as f64;
        let bh = (b.y1 - b.y0 + 1) as f64;
        let fill = b.area as f64 / (bw * bh);
        let aspect = bw.max(bh) / bw.min(bh);
        if !(0.55..=0.95).contains(&fill) || aspect > 3.0 {
            continue;
        }
        // Intensity-weighted centroid over the blob and a thin ring of anti-aliased pixels.
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        let xa = b.x0.saturating_sub(2);
        let xb = (b.x1 + 2).min(w - 1);
        let ya = b.y0.saturating_sub(2);
        let yb = (b.y1 + 2).min(h - 1);
        for y in ya..=yb {
            for x in xa..=xb {
                let l = labels[y * w + x];
                if l != 0 && l != b.id {
                    continue;
                }
                let wt = ((px(x, y) as f64 - level_off) / (level_on - level_off)).clamp(0.0, 1.0);
                let _ = span;
                sx += wt * x as f64;
                sy += wt * y as f64;
                sw += wt;
            }
        }
        if sw > 0.0 {
            out.push(Point::new(sx / sw, sy / sw));
        }
    }
    out
}

fn gaussian_blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let rad = (sigma * 3.0).ceil() as i32;
    let kern: Vec<f32> = (-rad..=rad).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let ks: f32 = kern.iter().sum();
    let kern: Vec<f32> = kern.iter().map(|k| k / ks).collect();
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, k) in kern.iter().enumerate() {
                let xx = (x as i32 + i as i32 - rad).clamp(0, w as i32 - 1) as usize;
                s += k * src[y * w + xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, k) in kern.iter().enumerate() {
                let yy = (y as i32 + i as i32 - rad).clamp(0, h as i32 - 1) as usize;
                s += k * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

fn bilinear(a: &[f32], w: usize, h: usize, x: f64, y: f64) -> f64 {
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let g = |xx: usize, yy: usize| a[yy * w + xx] as f64;
    (g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy) + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy
}

/// Checkerboard inner corners as saddle points of the smoothed image, strongest first, with their response.
pub fn detect_checkerboard_corners(img: &GrayImage) -> Vec<(Point, f64)> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    let raw: Vec<f32> = img.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    let sm = gaussian_blur(&raw, w, h, 1.6);
    let at = |x: usize, y: usize| sm[y * w + x];
    let mut gx = vec![0f32; w * h];
    let mut gy = vec![0f32; w * h];
    let mut hxx = vec![0f32; w * h];
    let mut hyy = vec![0f32; w * h];
    let mut hxy = vec![0f32; w * h];
    let mut resp = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            gx[i] = (at(x + 1, y) - at(x - 1, y)) * 0.5;
            gy[i] = (at(x, y + 1) - at(x, y - 1)) * 0.5;
            hxx[i] = at(x + 1, y) - 2.0 * at(x, y) + at(x - 1, y);
            hyy[i] = at(x, y + 1) - 2.0 * at(x, y) + at(x, y - 1);
            hxy[i] = (at(x + 1, y + 1) - at(x - 1, y + 1) - at(x + 1, y - 1) + at(x - 1, y - 1)) * 0.25;
            resp[i] = -(hxx[i] * hyy[i] - hxy[i] * hxy[i]);
        }
    }
    let rmax = resp.iter().cloned().fold(0.0f32, f32::max);
    if rmax <= 0.0 {
        return Vec::new();
    }
    let thr = rmax * 0.25;
    let rad = 4i32;
    let mut cand: Vec<(usize, usize, f32)> = Vec::new();
    for y in rad as usize..h - rad as usize {
        for x in rad as usize..w - rad as usize {
            let r = resp[y * w + x];
            if r < thr {
                continue;
            }
            let mut is_max = true;
            'win: for dy in -rad..=rad {
                for dx in -rad..=rad {
                    if dx != 0 || dy != 0 {
                        let q = resp[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
                        if q > r || (q == r && (dy < 0 || (dy == 0 && dx < 0))) {
                            is_max = false;
                            break 'win;
                        }
                    }
                }
            }
            if is_max {
                cand.push((x, y, r));
            }
        }
    }
    cand.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut out = Vec::new();
    for (x, y, r) in cand {
        let (mut px, mut py) = (x as f64, y as f64);
        for _ in 0..5 {
            let g = (bilinear(&gx, w, h, px, py), bilinear(&gy, w, h, px, py));
            let a = bilinear(&hxx, w, h, px, py);
            let b = bilinear(&hxy, w, h, px, py);
            let d = bilinear(&hyy, w, h, px, py);
            let det = a * d - b * b;
            if det.abs() < 1e-12 {
                break;
            }
            let dx = -(d * g.0 - b * g.1) / det;
            let dy = -(-b * g.0 + a * g.1) / det;
            px += dx.clamp(-1.0, 1.0);
            py += dy.clamp(-1.0, 1.0);
            if dx.abs() < 1e-4 && dy.abs() < 1e-4 {
                break;
            }
        }
        if (px - x as f64).abs() > 2.0 || (py - y as f64).abs() > 2.0 {
            px = x as f64;
            py = y as f64;
        }
        out.push((Point::new(px, py), r as f64));
    }
    out
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Convex hull (monotone chain), returned as points in counter-clockwise order in a y-up frame.
fn convex_hull(pts: &[Point]) -> Vec<Point> {
    let mut p: Vec<Point> = pts.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut hull: Vec<Point> = Vec::new();
    for &q in &p {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
            hull.pop();
        }
        hull.push(q);
    }
    let lower = hull.len() + 1;
    for &q in p.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], q) <= 0.0 {
            hull.pop();
        }
        hull.push(q);
    }
    hull.pop();
    hull
}

fn quad_area(q: [Point; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        s += a.x * b.y - b.x * a.y;
    }
    s * 0.5
}

/// Arrange an unordered set of `cols * rows` grid points into row-major order.
///
/// The first point is the grid corner whose neighbours run right and down in the image
/// (so a mirrored arrangement is never produced). Handles perspective and moderate lens distortion.
pub fn order_grid(points: &[Point], cols: usize, rows: usize) -> Option<Vec<Point>> {
    let n = cols * rows;
    if cols < 2 || rows < 2 || points.len() != n {
        return None;
    }
    let hull = convex_hull(points);
    let hn = hull.len();
    if hn < 4 {
        return None;
    }
    // Largest quadrilateral on the hull.
    let mut best: Option<([Point; 4], f64)> = None;
    for i in 0..hn {
        for j in i + 1..hn {
            for k in j + 1..hn {
                for l in k + 1..hn {
                    let q = [hull[i], hull[j], hull[k], hull[l]];
                    let a = quad_area(q).abs();
                    if best.as_ref().is_none_or(|b| a > b.1) {
                        best = Some((q, a));
                    }
                }
            }
        }
    }
    let (mut quad, _) = best?;
    // Ideal corners run (0,0) → (c-1,0) → (c-1,r-1) → (0,r-1): positive signed area in a y-down frame as well.
    if quad_area(quad) < 0.0 {
        quad.reverse();
    }
    let ideal: Vec<Point> = (0..rows).flat_map(|r| (0..cols).map(move |c| Point::new(c as f64, r as f64))).collect();
    let ideal_corners = [
        Point::new(0.0, 0.0),
        Point::new((cols - 1) as f64, 0.0),
        Point::new((cols - 1) as f64, (rows - 1) as f64),
        Point::new(0.0, (rows - 1) as f64),
    ];

    let mut best_result: Option<(Vec<Point>, f64)> = None;
    for shift in 0..4 {
        let img_corners: Vec<Point> = (0..4).map(|i| quad[(i + shift) % 4]).collect();
        let Ok(mut hm) = fit_homography_dlt(&ideal_corners, &img_corners) else { continue };
        let mut assignment: Option<Vec<usize>> = None;
        let mut ok = true;
        for pass in 0..3 {
            let Some(a) = match_points(&hm, &ideal, points, if pass == 0 { 0.5 } else { 0.4 }) else {
                ok = false;
                break;
            };
            let src = ideal.clone();
            let dst: Vec<Point> = a.iter().map(|&i| points[i]).collect();
            match fit_homography_dlt(&src, &dst) {
                Ok(h) => hm = h,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
            assignment = Some(a);
        }
        if !ok {
            continue;
        }
        let a = assignment?;
        let ordered: Vec<Point> = a.iter().map(|&i| points[i]).collect();
        let err = hm.rms_error(&ideal, &ordered);
        if best_result.as_ref().is_none_or(|b| err < b.1) {
            best_result = Some((ordered, err));
        }
    }
    best_result.map(|b| b.0)
}

/// Assign each ideal grid point to the nearest image point; `None` unless it is a one-to-one match within tolerance.
fn match_points(h: &Homography, ideal: &[Point], pts: &[Point], tol_frac: f64) -> Option<Vec<usize>> {
    let proj: Vec<Point> = ideal.iter().map(|p| h.apply(*p)).collect::<Option<Vec<_>>>()?;
    // Smallest projected neighbour spacing.
    let mut spacing = f64::INFINITY;
    for (i, a) in proj.iter().enumerate() {
        for b in proj.iter().skip(i + 1) {
            spacing = spacing.min((*a - *b).hypot());
        }
    }
    let tol = spacing * tol_frac;
    let mut used = vec![false; pts.len()];
    let mut out = Vec::with_capacity(ideal.len());
    for p in &proj {
        let (mut bi, mut bd) = (usize::MAX, f64::INFINITY);
        for (i, q) in pts.iter().enumerate() {
            let d = (*p - *q).hypot();
            if d < bd {
                bd = d;
                bi = i;
            }
        }
        if bi == usize::MAX || bd > tol || used[bi] {
            return None;
        }
        used[bi] = true;
        out.push(bi);
    }
    Some(out)
}
