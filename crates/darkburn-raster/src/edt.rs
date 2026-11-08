//! Exact Euclidean distance transform (the separable lower-envelope method of Felzenszwalb &
//! Huttenlocher, "Distance Transforms of Sampled Functions", 2012), written from the paper.

const INF: f32 = 1e20;

/// Squared distance from each pixel centre to the nearest pixel where `feature` is true.
/// Pixels with no feature anywhere get a very large value.
pub fn distance_transform_sq(feature: &[bool], width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(feature.len(), w * h);
    let mut grid: Vec<f32> = feature.iter().map(|&f| if f { 0.0 } else { INF }).collect();
    let n = w.max(h);
    let mut f = vec![0.0f32; n];
    let mut d = vec![0.0f32; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f32; n + 1];
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        dt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        dt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    grid
}

fn dt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;
    for q in 1..n {
        let fq = f[q] + (q * q) as f32;
        loop {
            let p = v[k];
            let s = (fq - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else if s <= z[k] {
                // k == 0: replace the only parabola.
                v[0] = q;
                z[0] = -INF;
                z[1] = INF;
                break;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
                break;
            }
        }
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        let diff = q as f32 - p as f32;
        *dq = diff * diff + f[p];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_brute_force() {
        let (w, h) = (13u32, 9u32);
        let feature: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 17 == 3).collect();
        let d = distance_transform_sq(&feature, w, h);
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut best = INF;
                for fy in 0..h as i64 {
                    for fx in 0..w as i64 {
                        if feature[(fy * w as i64 + fx) as usize] {
                            best = best.min(((x - fx).pow(2) + (y - fy).pow(2)) as f32);
                        }
                    }
                }
                assert_eq!(d[(y * w as i64 + x) as usize], best, "at {x},{y}");
            }
        }
    }

    #[test]
    fn no_features_is_far() {
        let d = distance_transform_sq(&[false; 4], 2, 2);
        assert!(d.iter().all(|&v| v >= INF));
    }
}
