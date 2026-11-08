//! Synthetic test data: render a calibration pattern through a lens model.

use crate::calibrate::{Pose, rot_to_rvec};
use crate::detect::{PatternKind, PatternSpec};
use crate::lens::LensModel;
use image::{GrayImage, Luma};
use kurbo::Point;

fn rot_xyz(ax: f64, ay: f64, az: f64) -> [[f64; 3]; 3] {
    let (sx, cx) = ax.to_radians().sin_cos();
    let (sy, cy) = ay.to_radians().sin_cos();
    let (sz, cz) = az.to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut o = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    mul(mul(rx, ry), rz)
}

/// Pose with the board centre at `centre` (camera space, mm) after tilting by the given angles in degrees.
pub fn pose_for(cols: usize, rows: usize, spacing: f64, tilt: (f64, f64, f64), centre: (f64, f64, f64)) -> Pose {
    let r = rot_xyz(tilt.0, tilt.1, tilt.2);
    let (hw, hh) = ((cols - 1) as f64 * spacing / 2.0, (rows - 1) as f64 * spacing / 2.0);
    let t = [
        centre.0 - (r[0][0] * hw + r[0][1] * hh),
        centre.1 - (r[1][0] * hw + r[1][1] * hh),
        centre.2 - (r[2][0] * hw + r[2][1] * hh),
    ];
    Pose { rvec: rot_to_rvec(&r), tvec: t }
}

/// A spread of tilted, off-centre views for a 1280x720 camera at ~900 px focal length.
pub fn sample_poses(cols: usize, rows: usize, spacing: f64) -> Vec<Pose> {
    let v = [
        ((20.0, 0.0, 0.0), (0.0, 0.0, 380.0)),
        ((-20.0, 10.0, 5.0), (-110.0, -50.0, 380.0)),
        ((0.0, 25.0, -5.0), (110.0, -40.0, 400.0)),
        ((15.0, -25.0, 10.0), (-100.0, 50.0, 360.0)),
        ((-25.0, -15.0, 0.0), (110.0, 55.0, 380.0)),
        ((30.0, 20.0, 15.0), (0.0, 60.0, 400.0)),
        ((10.0, 30.0, -10.0), (-40.0, -60.0, 370.0)),
        ((-30.0, 20.0, 5.0), (40.0, 10.0, 420.0)),
    ];
    v.iter().map(|(t, c)| pose_for(cols, rows, spacing, *t, *c)).collect()
}

fn pattern_value(spec: &PatternSpec, x: f64, y: f64) -> u8 {
    let s = spec.spacing_mm;
    match spec.kind {
        PatternKind::CircleGrid { dark_on_light } => {
            let c = (x / s).round().clamp(0.0, (spec.cols - 1) as f64);
            let r = (y / s).round().clamp(0.0, (spec.rows - 1) as f64);
            let d = ((x - c * s).powi(2) + (y - r * s).powi(2)).sqrt();
            let inside = d < 0.3 * s;
            if inside == dark_on_light { 0 } else { 255 }
        }
        PatternKind::Checkerboard => {
            let i = (x / s).floor() as i64;
            let j = (y / s).floor() as i64;
            if i < -1 || j < -1 || i > spec.cols as i64 - 1 || j > spec.rows as i64 - 1 {
                return 255;
            }
            if (i + j).rem_euclid(2) == 0 { 0 } else { 255 }
        }
    }
}

/// Render the pattern seen by `lens` for a given pose (3x3 supersampling inside the board's bounding box).
pub fn render_pattern(lens: &LensModel, pose: &Pose, spec: &PatternSpec) -> GrayImage {
    let mut img = GrayImage::from_pixel(lens.width, lens.height, Luma([255]));
    let rm = {
        // rebuild the rotation matrix from the Rodrigues vector
        let r = pose.rvec;
        let th = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        if th < 1e-12 {
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        } else {
            let (kx, ky, kz) = (r[0] / th, r[1] / th, r[2] / th);
            let (s, c) = th.sin_cos();
            let v = 1.0 - c;
            [
                [c + kx * kx * v, kx * ky * v - kz * s, kx * kz * v + ky * s],
                [ky * kx * v + kz * s, c + ky * ky * v, ky * kz * v - kx * s],
                [kz * kx * v - ky * s, kz * ky * v + kx * s, c + kz * kz * v],
            ]
        }
    };
    let t = pose.tvec;
    let to_cam = |x: f64, y: f64| {
        [rm[0][0] * x + rm[0][1] * y + t[0], rm[1][0] * x + rm[1][1] * y + t[1], rm[2][0] * x + rm[2][1] * y + t[2]]
    };
    // bounding box of the board with a margin, in image space
    let s = spec.spacing_mm;
    let (x0, x1, y0, y1) = (-1.6 * s, (spec.cols as f64 - 1.0 + 1.6) * s, -1.6 * s, (spec.rows as f64 - 1.0 + 1.6) * s);
    let (mut bx0, mut bx1, mut by0, mut by1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for i in 0..=10 {
        for j in 0..=10 {
            let c = to_cam(x0 + (x1 - x0) * i as f64 / 10.0, y0 + (y1 - y0) * j as f64 / 10.0);
            if let Some(p) = lens.project(c[0], c[1], c[2]) {
                bx0 = bx0.min(p.x);
                bx1 = bx1.max(p.x);
                by0 = by0.min(p.y);
                by1 = by1.max(p.y);
            }
        }
    }
    let (px0, px1) = ((bx0 - 4.0).max(0.0) as u32, ((bx1 + 4.0) as u32).min(lens.width - 1));
    let (py0, py1) = ((by0 - 4.0).max(0.0) as u32, ((by1 + 4.0) as u32).min(lens.height - 1));
    let n = [rm[0][2], rm[1][2], rm[2][2]];
    let nt = n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
    for j in py0..=py1 {
        for i in px0..=px1 {
            let mut acc = 0.0;
            for sj in 0..3 {
                for si in 0..3 {
                    let p = Point::new(i as f64 + (si as f64 - 1.0) / 3.0, j as f64 + (sj as f64 - 1.0) / 3.0);
                    let (nx, ny) = lens.pixel_to_normalized(p);
                    let nd = n[0] * nx + n[1] * ny + n[2];
                    let v = if nd.abs() < 1e-9 {
                        255
                    } else {
                        let sc = nt / nd;
                        let pc = [sc * nx - t[0], sc * ny - t[1], sc - t[2]];
                        let u = rm[0][0] * pc[0] + rm[1][0] * pc[1] + rm[2][0] * pc[2];
                        let w = rm[0][1] * pc[0] + rm[1][1] * pc[1] + rm[2][1] * pc[2];
                        pattern_value(spec, u, w)
                    };
                    acc += v as f64;
                }
            }
            img.put_pixel(i, j, Luma([(acc / 9.0).round() as u8]));
        }
    }
    img
}
