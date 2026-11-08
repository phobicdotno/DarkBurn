//! Lens calibration from several views of a flat grid (Zhang's method, public): closed-form start, then
//! Levenberg-Marquardt over intrinsics, distortion and per-view poses.

use crate::detect::{PatternSpec, detect_pattern};
use crate::homography::fit_homography;
use crate::lens::{LensKind, LensModel};
use crate::linalg::{LmOptions, Matrix, levenberg_marquardt, null_vector, sym_eigen};
use image::GrayImage;
use kurbo::Point;
use serde::{Deserialize, Serialize};

/// Pose of the target plane in camera space (Rodrigues rotation vector and translation, mm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub rvec: [f64; 3],
    pub tvec: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub enum CalibError {
    /// At least three usable views are needed.
    NotEnoughViews { usable: usize },
    /// A view has the wrong number of points.
    BadView { index: usize },
    Degenerate,
}

impl std::fmt::Display for CalibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughViews { usable } => write!(f, "need at least 3 usable views, got {usable}"),
            Self::BadView { index } => write!(f, "view {index} has the wrong number of points"),
            Self::Degenerate => write!(f, "views are degenerate (tilt the target between captures)"),
        }
    }
}
impl std::error::Error for CalibError {}

/// What to solve for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibOptions {
    pub kind: LensKind,
    /// Number of radial coefficients to solve for (1..=3 standard, 1..=4 fisheye).
    pub num_distortion: usize,
    /// Tie `fy` to `fx` (square pixels).
    pub fix_aspect_ratio: bool,
    /// Keep the principal point at the image centre.
    pub fix_principal_point: bool,
}

impl Default for CalibOptions {
    fn default() -> Self {
        Self { kind: LensKind::Standard, num_distortion: 2, fix_aspect_ratio: true, fix_principal_point: false }
    }
}

/// Outcome of a calibration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationResult {
    pub model: LensModel,
    /// Root mean square reprojection error over all points, pixels.
    pub rms_px: f64,
    /// Per-view RMS reprojection error, pixels.
    pub view_rms_px: Vec<f64>,
    pub poses: Vec<Pose>,
}

type M3 = [[f64; 3]; 3];

pub(crate) fn rodrigues(r: [f64; 3]) -> M3 {
    let th = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    if th < 1e-12 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let (kx, ky, kz) = (r[0] / th, r[1] / th, r[2] / th);
    let (s, c) = th.sin_cos();
    let v = 1.0 - c;
    [
        [c + kx * kx * v, kx * ky * v - kz * s, kx * kz * v + ky * s],
        [ky * kx * v + kz * s, c + ky * ky * v, ky * kz * v - kx * s],
        [kz * kx * v - ky * s, kz * ky * v + kx * s, c + kz * kz * v],
    ]
}

pub(crate) fn rot_to_rvec(m: &M3) -> [f64; 3] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let th = ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos();
    if th < 1e-9 {
        return [0.0; 3];
    }
    if std::f64::consts::PI - th < 1e-4 {
        let x = ((m[0][0] + 1.0) / 2.0).max(0.0).sqrt();
        let y = ((m[1][1] + 1.0) / 2.0).max(0.0).sqrt() * if m[0][1] + m[1][0] < 0.0 { -1.0 } else { 1.0 };
        let z = ((m[2][2] + 1.0) / 2.0).max(0.0).sqrt() * if m[0][2] + m[2][0] < 0.0 { -1.0 } else { 1.0 };
        let n = (x * x + y * y + z * z).sqrt().max(1e-12);
        return [x / n * th, y / n * th, z / n * th];
    }
    let k = th / (2.0 * th.sin());
    [(m[2][1] - m[1][2]) * k, (m[0][2] - m[2][0]) * k, (m[1][0] - m[0][1]) * k]
}

/// Nearest rotation matrix (polar decomposition).
fn orthonormalise(m: &M3) -> M3 {
    let mut a = Matrix::zeros(3, 3);
    for i in 0..3 {
        for j in 0..3 {
            a.set(i, j, m[i][j]);
        }
    }
    let g = a.gram();
    let (vals, vecs) = sym_eigen(&g);
    // (MᵀM)^(-1/2)
    let mut inv_sqrt = Matrix::zeros(3, 3);
    for k in 0..3 {
        let s = 1.0 / vals[k].max(1e-12).sqrt();
        for i in 0..3 {
            for j in 0..3 {
                inv_sqrt.add(i, j, s * vecs.get(i, k) * vecs.get(j, k));
            }
        }
    }
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| m[i][k] * inv_sqrt.get(k, j)).sum();
        }
    }
    out
}

fn transform(r: &M3, t: [f64; 3], p: [f64; 3]) -> [f64; 3] {
    [
        r[0][0] * p[0] + r[0][1] * p[1] + r[0][2] * p[2] + t[0],
        r[1][0] * p[0] + r[1][1] * p[1] + r[1][2] * p[2] + t[1],
        r[2][0] * p[0] + r[2][1] * p[1] + r[2][2] * p[2] + t[2],
    ]
}

/// Project target-plane points (mm) with a pose through the lens.
pub fn project_target(model: &LensModel, pose: &Pose, plane: &[Point]) -> Vec<Option<Point>> {
    let r = rodrigues(pose.rvec);
    plane
        .iter()
        .map(|p| {
            let c = transform(&r, pose.tvec, [p.x, p.y, 0.0]);
            model.project(c[0], c[1], c[2])
        })
        .collect()
}

fn zhang_intrinsics(homs: &[[f64; 9]], w: u32, h: u32) -> (f64, f64, f64, f64) {
    let fallback = {
        let f = w.max(h) as f64;
        (f, f, (w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0)
    };
    let v = |hm: &[f64; 9], i: usize, j: usize| {
        let a = |k: usize| hm[k * 3 + i];
        let b = |k: usize| hm[k * 3 + j];
        [a(0) * b(0), a(0) * b(1) + a(1) * b(0), a(1) * b(1), a(2) * b(0) + a(0) * b(2), a(2) * b(1) + a(1) * b(2), a(2) * b(2)]
    };
    let mut a = Matrix::zeros(homs.len() * 2 + 1, 6);
    for (n, hm) in homs.iter().enumerate() {
        let v12 = v(hm, 0, 1);
        let v11 = v(hm, 0, 0);
        let v22 = v(hm, 1, 1);
        for c in 0..6 {
            a.set(2 * n, c, v12[c]);
            a.set(2 * n + 1, c, v11[c] - v22[c]);
        }
    }
    // zero skew
    let last = homs.len() * 2;
    a.set(last, 1, 1.0);
    let b = null_vector(&a);
    let (mut b11, mut b12, mut b22, mut b13, mut b23, mut b33) = (b[0], b[1], b[2], b[3], b[4], b[5]);
    if b11 < 0.0 {
        b11 = -b11;
        b12 = -b12;
        b22 = -b22;
        b13 = -b13;
        b23 = -b23;
        b33 = -b33;
    }
    let den = b11 * b22 - b12 * b12;
    if den.abs() < 1e-30 || b11.abs() < 1e-30 {
        return fallback;
    }
    let v0 = (b12 * b13 - b11 * b23) / den;
    let lambda = b33 - (b13 * b13 + v0 * (b12 * b13 - b11 * b23)) / b11;
    let alpha2 = lambda / b11;
    let beta2 = lambda * b11 / den;
    if !(alpha2 > 0.0 && beta2 > 0.0) {
        return fallback;
    }
    let (alpha, beta) = (alpha2.sqrt(), beta2.sqrt());
    let u0 = -b13 * alpha2 / lambda;
    if ![alpha, beta, u0, v0].iter().all(|x| x.is_finite()) {
        return fallback;
    }
    (alpha, beta, u0, v0)
}

fn initial_pose(hm: &[f64; 9], fx: f64, fy: f64, cx: f64, cy: f64) -> Pose {
    // K⁻¹ H
    let kinv = |col: [f64; 3]| [(col[0] - cx * col[2]) / fx, (col[1] - cy * col[2]) / fy, col[2]];
    let c = |j: usize| [hm[j], hm[3 + j], hm[6 + j]];
    let (a1, a2, a3) = (kinv(c(0)), kinv(c(1)), kinv(c(2)));
    let norm = (a1[0] * a1[0] + a1[1] * a1[1] + a1[2] * a1[2]).sqrt().max(1e-12);
    let mut lam = 1.0 / norm;
    // Keep the target in front of the camera.
    if a3[2] * lam < 0.0 {
        lam = -lam;
    }
    let r1 = [a1[0] * lam, a1[1] * lam, a1[2] * lam];
    let r2 = [a2[0] * lam, a2[1] * lam, a2[2] * lam];
    let r3 = [r1[1] * r2[2] - r1[2] * r2[1], r1[2] * r2[0] - r1[0] * r2[2], r1[0] * r2[1] - r1[1] * r2[0]];
    let r = orthonormalise(&[[r1[0], r2[0], r3[0]], [r1[1], r2[1], r3[1]], [r1[2], r2[2], r3[2]]]);
    Pose { rvec: rot_to_rvec(&r), tvec: [a3[0] * lam, a3[1] * lam, a3[2] * lam] }
}

/// Solve the lens model from ordered grid detections (one `Vec<Point>` of `plane.len()` points per view).
pub fn calibrate(plane: &[Point], views: &[Vec<Point>], image_size: (u32, u32), opts: CalibOptions) -> Result<CalibrationResult, CalibError> {
    if views.len() < 3 {
        return Err(CalibError::NotEnoughViews { usable: views.len() });
    }
    for (i, v) in views.iter().enumerate() {
        if v.len() != plane.len() {
            return Err(CalibError::BadView { index: i });
        }
    }
    let (w, h) = image_size;
    let homs: Vec<[f64; 9]> =
        views.iter().map(|v| fit_homography(plane, v).map(|h| h.m)).collect::<Result<_, _>>().map_err(|_| CalibError::Degenerate)?;
    let (fx0, fy0, cx0, cy0) = zhang_intrinsics(&homs, w, h);
    let (cx0, cy0) = if opts.fix_principal_point { ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0) } else { (cx0, cy0) };
    let f0 = if opts.fix_aspect_ratio { (fx0 + fy0) / 2.0 } else { fx0 };
    let poses0: Vec<Pose> = homs.iter().map(|hm| initial_pose(hm, f0, if opts.fix_aspect_ratio { f0 } else { fy0 }, cx0, cy0)).collect();

    let max_k = if opts.kind == LensKind::Fisheye { 4 } else { 3 };
    let nk = opts.num_distortion.clamp(1, max_k);

    let mut init = vec![f0];
    if !opts.fix_aspect_ratio {
        init.push(fy0);
    }
    if !opts.fix_principal_point {
        init.push(cx0);
        init.push(cy0);
    }
    let n_intr = init.len() + nk;
    init.extend(std::iter::repeat_n(0.0, nk));
    for p in &poses0 {
        init.extend_from_slice(&p.rvec);
        init.extend_from_slice(&p.tvec);
    }

    let build = |p: &[f64]| -> LensModel {
        let mut i = 0;
        let fx = p[i];
        i += 1;
        let fy = if opts.fix_aspect_ratio {
            fx
        } else {
            i += 1;
            p[i - 1]
        };
        let (cx, cy) = if opts.fix_principal_point {
            ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0)
        } else {
            i += 2;
            (p[i - 2], p[i - 1])
        };
        let mut k = [0.0; 4];
        for j in 0..nk {
            k[j] = p[i + j];
        }
        LensModel { name: String::new(), kind: opts.kind, width: w, height: h, fx, fy, cx, cy, k }
    };
    let pose_of = |p: &[f64], v: usize| {
        let o = n_intr + v * 6;
        Pose { rvec: [p[o], p[o + 1], p[o + 2]], tvec: [p[o + 3], p[o + 4], p[o + 5]] }
    };
    let rots = |p: &[f64]| -> Vec<M3> { (0..views.len()).map(|v| rodrigues(pose_of(p, v).rvec)).collect() };

    let residuals = |p: &[f64]| -> Vec<f64> {
        let model = build(p);
        let rs = rots(p);
        let mut out = Vec::with_capacity(views.len() * plane.len() * 2);
        for (v, obs) in views.iter().enumerate() {
            let t = pose_of(p, v).tvec;
            for (pt, o) in plane.iter().zip(obs) {
                let c = transform(&rs[v], t, [pt.x, pt.y, 0.0]);
                match model.project(c[0], c[1], c[2]) {
                    Some(q) => {
                        out.push(q.x - o.x);
                        out.push(q.y - o.y);
                    }
                    None => {
                        out.push(1e4);
                        out.push(1e4);
                    }
                }
            }
        }
        out
    };

    let res = levenberg_marquardt(&init, residuals, LmOptions { max_iterations: 200, tolerance: 1e-13 });
    let model = build(&res.params);
    if !(model.fx.is_finite() && model.fx > 0.0 && model.fy > 0.0) {
        return Err(CalibError::Degenerate);
    }
    let poses: Vec<Pose> = (0..views.len()).map(|v| pose_of(&res.params, v)).collect();
    let mut view_rms = Vec::new();
    let mut total = 0.0;
    for (v, obs) in views.iter().enumerate() {
        let proj = project_target(&model, &poses[v], plane);
        let mut s = 0.0;
        for (q, o) in proj.iter().zip(obs) {
            s += q.map_or(1e8, |q| (q - *o).hypot2());
        }
        total += s;
        view_rms.push((s / plane.len() as f64).sqrt());
    }
    let rms = (total / (views.len() * plane.len()) as f64).sqrt();
    Ok(CalibrationResult { model, rms_px: rms, view_rms_px: view_rms, poses })
}

/// Detect the pattern in each image and calibrate. Images where detection fails are skipped and listed.
pub fn calibrate_from_images(spec: &PatternSpec, images: &[GrayImage], opts: CalibOptions) -> Result<(CalibrationResult, Vec<usize>), CalibError> {
    let Some(first) = images.first() else { return Err(CalibError::NotEnoughViews { usable: 0 }) };
    let size = (first.width(), first.height());
    let mut views = Vec::new();
    let mut skipped = Vec::new();
    for (i, img) in images.iter().enumerate() {
        match detect_pattern(img, spec) {
            Some(p) => views.push(p),
            None => skipped.push(i),
        }
    }
    let res = calibrate(&spec.object_points(), &views, size, opts)?;
    Ok((res, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::PatternKind;
    use crate::testutil::{render_pattern, sample_poses};

    fn truth_standard() -> LensModel {
        LensModel { fx: 900.0, fy: 900.0, cx: 645.0, cy: 355.0, k: [-0.25, 0.06, 0.0, 0.0], ..LensModel::ideal(1280, 720, 900.0) }
    }

    fn spec() -> PatternSpec {
        PatternSpec { kind: PatternKind::CircleGrid { dark_on_light: true }, cols: 7, rows: 5, spacing_mm: 25.0 }
    }

    #[test]
    fn rodrigues_roundtrip() {
        for r in [[0.1, -0.2, 0.3], [1.0, 0.5, -0.2], [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]] {
            let m = rodrigues(r);
            let back = rot_to_rvec(&m);
            for i in 0..3 {
                assert!((back[i] - r[i]).abs() < 1e-9, "{r:?} {back:?}");
            }
        }
    }

    #[test]
    fn solves_exact_standard_model_from_points() {
        let truth = truth_standard();
        let sp = spec();
        let plane = sp.object_points();
        let views: Vec<Vec<Point>> = sample_poses(sp.cols, sp.rows, sp.spacing_mm).iter().map(|p| project_target(&truth, p, &plane).into_iter().map(|q| q.unwrap()).collect()).collect();
        let res = calibrate(&plane, &views, (1280, 720), CalibOptions::default()).unwrap();
        assert!(res.rms_px < 1e-3, "rms {}", res.rms_px);
        assert!((res.model.fx - 900.0).abs() < 0.5, "{:?}", res.model);
        assert!((res.model.cx - 645.0).abs() < 0.5 && (res.model.cy - 355.0).abs() < 0.5);
        assert!((res.model.k[0] + 0.25).abs() < 0.005 && (res.model.k[1] - 0.06).abs() < 0.02);
    }

    #[test]
    fn solves_fisheye_from_points() {
        let truth = LensModel { kind: LensKind::Fisheye, fx: 700.0, fy: 700.0, cx: 640.0, cy: 360.0, k: [-0.03, 0.01, 0.0, 0.0], ..LensModel::ideal(1280, 720, 700.0) };
        let sp = spec();
        let plane = sp.object_points();
        let views: Vec<Vec<Point>> = sample_poses(sp.cols, sp.rows, sp.spacing_mm).iter().map(|p| project_target(&truth, p, &plane).into_iter().map(|q| q.unwrap()).collect()).collect();
        let res = calibrate(&plane, &views, (1280, 720), CalibOptions { kind: LensKind::Fisheye, ..Default::default() }).unwrap();
        assert!(res.rms_px < 0.05, "rms {}", res.rms_px);
        assert!((res.model.fx - 700.0).abs() < 3.0, "{:?}", res.model);
        // Undistorting a point lands where the truth model puts it.
        let p = Point::new(100.0, 90.0);
        assert!((res.model.undistort_point(p) - truth.undistort_point(p)).hypot() < 1.0);
    }

    #[test]
    fn calibrates_from_rendered_images() {
        let truth = truth_standard();
        let sp = spec();
        let images: Vec<GrayImage> = sample_poses(sp.cols, sp.rows, sp.spacing_mm).iter().map(|p| render_pattern(&truth, p, &sp)).collect();
        let (res, skipped) = calibrate_from_images(&sp, &images, CalibOptions::default()).unwrap();
        assert!(skipped.is_empty(), "skipped {skipped:?}");
        assert!(res.rms_px < 0.5, "rms {}", res.rms_px);
        assert!((res.model.fx - 900.0).abs() / 900.0 < 0.02, "{:?}", res.model);
        assert!((res.model.cx - 645.0).abs() < 6.0 && (res.model.cy - 355.0).abs() < 6.0);
    }

    #[test]
    fn calibrates_from_checkerboard_images() {
        let truth = truth_standard();
        let sp = PatternSpec { kind: PatternKind::Checkerboard, cols: 7, rows: 5, spacing_mm: 25.0 };
        let images: Vec<GrayImage> = sample_poses(sp.cols, sp.rows, sp.spacing_mm).iter().map(|p| render_pattern(&truth, p, &sp)).collect();
        let (res, skipped) = calibrate_from_images(&sp, &images, CalibOptions::default()).unwrap();
        assert!(skipped.is_empty(), "skipped {skipped:?}");
        assert!(res.rms_px < 0.5, "rms {}", res.rms_px);
        assert!((res.model.fx - 900.0).abs() / 900.0 < 0.02, "{:?}", res.model);
    }

    #[test]
    fn rejects_too_few_views() {
        let sp = spec();
        let plane = sp.object_points();
        let err = calibrate(&plane, &[plane.clone(), plane.clone()], (640, 480), CalibOptions::default()).unwrap_err();
        assert_eq!(err, CalibError::NotEnoughViews { usable: 2 });
        let err = calibrate(&plane, &[plane.clone(), plane.clone(), vec![]], (640, 480), CalibOptions::default()).unwrap_err();
        assert_eq!(err, CalibError::BadView { index: 2 });
    }
}
