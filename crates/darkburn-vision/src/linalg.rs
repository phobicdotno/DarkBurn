//! Small dense linear algebra: enough for homographies, Zhang calibration and Levenberg-Marquardt.

/// Row-major dense matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Matrix {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self { rows, cols, data: vec![0.0; rows * cols] }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m.data[i * n + i] = 1.0;
        }
        m
    }

    #[inline]
    pub fn get(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    #[inline]
    pub fn set(&mut self, r: usize, c: usize, v: f64) {
        self.data[r * self.cols + c] = v;
    }

    #[inline]
    pub fn add(&mut self, r: usize, c: usize, v: f64) {
        self.data[r * self.cols + c] += v;
    }

    /// `selfᵀ · self`.
    pub fn gram(&self) -> Matrix {
        let n = self.cols;
        let mut g = Matrix::zeros(n, n);
        for r in 0..self.rows {
            let row = &self.data[r * n..(r + 1) * n];
            for i in 0..n {
                let ri = row[i];
                if ri == 0.0 {
                    continue;
                }
                for j in i..n {
                    g.data[i * n + j] += ri * row[j];
                }
            }
        }
        for i in 0..n {
            for j in 0..i {
                g.data[i * n + j] = g.data[j * n + i];
            }
        }
        g
    }

    /// `selfᵀ · v`.
    pub fn t_mul_vec(&self, v: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; self.cols];
        for r in 0..self.rows {
            let vr = v[r];
            for c in 0..self.cols {
                out[c] += self.data[r * self.cols + c] * vr;
            }
        }
        out
    }
}

/// Solve `A x = b` by Gaussian elimination with partial pivoting. `None` if singular.
pub fn solve(a: &Matrix, b: &[f64]) -> Option<Vec<f64>> {
    let n = a.rows;
    if a.cols != n || b.len() != n {
        return None;
    }
    let mut m = a.data.clone();
    let mut x = b.to_vec();
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if m[r * n + col].abs() > m[piv * n + col].abs() {
                piv = r;
            }
        }
        if m[piv * n + col].abs() < 1e-300 {
            return None;
        }
        if piv != col {
            for c in 0..n {
                m.swap(col * n + c, piv * n + c);
            }
            x.swap(col, piv);
        }
        let d = m[col * n + col];
        for r in col + 1..n {
            let f = m[r * n + col] / d;
            if f == 0.0 {
                continue;
            }
            for c in col..n {
                m[r * n + c] -= f * m[col * n + c];
            }
            x[r] -= f * x[col];
        }
    }
    for col in (0..n).rev() {
        let mut s = x[col];
        for c in col + 1..n {
            s -= m[col * n + c] * x[c];
        }
        x[col] = s / m[col * n + col];
    }
    if x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(x)
}

/// Eigen-decomposition of a symmetric matrix by cyclic Jacobi.
/// Returns eigenvalues (ascending) and the matching eigenvectors as columns.
pub fn sym_eigen(a: &Matrix) -> (Vec<f64>, Matrix) {
    let n = a.rows;
    let mut m = a.clone();
    let mut v = Matrix::identity(n);
    let scale: f64 = a.data.iter().map(|x| x * x).sum::<f64>().max(1e-300);
    for _ in 0..100 {
        let mut off = 0.0;
        for i in 0..n {
            for j in 0..n {
                if i != j {
                    off += m.get(i, j) * m.get(i, j);
                }
            }
        }
        if off < 1e-26 * scale {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m.get(p, q);
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (m.get(q, q) - m.get(p, p)) / (2.0 * apq);
                let t = if theta == 0.0 { 1.0 } else { theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt()) };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = m.get(k, p);
                    let akq = m.get(k, q);
                    m.set(k, p, c * akp - s * akq);
                    m.set(k, q, s * akp + c * akq);
                }
                for k in 0..n {
                    let apk = m.get(p, k);
                    let aqk = m.get(q, k);
                    m.set(p, k, c * apk - s * aqk);
                    m.set(q, k, s * apk + c * aqk);
                }
                for k in 0..n {
                    let vkp = v.get(k, p);
                    let vkq = v.get(k, q);
                    v.set(k, p, c * vkp - s * vkq);
                    v.set(k, q, s * vkp + c * vkq);
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| m.get(i, i).total_cmp(&m.get(j, j)));
    let vals = idx.iter().map(|&i| m.get(i, i)).collect();
    let mut vecs = Matrix::zeros(n, n);
    for (new, &old) in idx.iter().enumerate() {
        for k in 0..n {
            vecs.set(k, new, v.get(k, old));
        }
    }
    (vals, vecs)
}

/// Unit vector `x` minimising `|A x|` (right singular vector of the smallest singular value).
pub fn null_vector(a: &Matrix) -> Vec<f64> {
    let (_, vecs) = sym_eigen(&a.gram());
    (0..a.cols).map(|k| vecs.get(k, 0)).collect()
}

/// Options for [`levenberg_marquardt`].
#[derive(Clone, Copy, Debug)]
pub struct LmOptions {
    pub max_iterations: usize,
    pub tolerance: f64,
}

impl Default for LmOptions {
    fn default() -> Self {
        Self { max_iterations: 100, tolerance: 1e-12 }
    }
}

/// Result of an LM run.
#[derive(Clone, Debug)]
pub struct LmResult {
    pub params: Vec<f64>,
    /// Final sum of squared residuals.
    pub cost: f64,
    pub iterations: usize,
}

fn sumsq(r: &[f64]) -> f64 {
    r.iter().map(|v| v * v).sum()
}

/// Minimise `|f(p)|²` with Levenberg-Marquardt and a central-difference Jacobian.
pub fn levenberg_marquardt<F: FnMut(&[f64]) -> Vec<f64>>(initial: &[f64], mut f: F, opts: LmOptions) -> LmResult {
    let n = initial.len();
    let mut p = initial.to_vec();
    let mut r = f(&p);
    let mut cost = sumsq(&r);
    let mut lambda = 1e-3;
    let mut iterations = 0;
    for it in 0..opts.max_iterations {
        iterations = it + 1;
        let m = r.len();
        let mut jac = Matrix::zeros(m, n);
        for j in 0..n {
            let h = 1e-6 * p[j].abs().max(1.0);
            let mut pp = p.clone();
            pp[j] = p[j] + h;
            let rp = f(&pp);
            pp[j] = p[j] - h;
            let rm = f(&pp);
            for i in 0..m {
                jac.set(i, j, (rp[i] - rm[i]) / (2.0 * h));
            }
        }
        let jtj = jac.gram();
        let g = jac.t_mul_vec(&r);
        let mut improved = false;
        for _ in 0..12 {
            let mut a = jtj.clone();
            for i in 0..n {
                let d = jtj.get(i, i).max(1e-12);
                a.add(i, i, lambda * d);
            }
            let neg: Vec<f64> = g.iter().map(|v| -v).collect();
            let Some(step) = solve(&a, &neg) else {
                lambda *= 10.0;
                continue;
            };
            let cand: Vec<f64> = p.iter().zip(&step).map(|(a, b)| a + b).collect();
            let rc = f(&cand);
            let cc = sumsq(&rc);
            if cc.is_finite() && cc < cost {
                let rel = (cost - cc) / cost.max(1e-300);
                p = cand;
                r = rc;
                cost = cc;
                lambda = (lambda * 0.3).max(1e-12);
                improved = true;
                if rel < opts.tolerance {
                    return LmResult { params: p, cost, iterations };
                }
                break;
            }
            lambda *= 10.0;
        }
        if !improved || cost < 1e-24 {
            break;
        }
    }
    LmResult { params: p, cost, iterations }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_system() {
        let a = Matrix { rows: 2, cols: 2, data: vec![2.0, 1.0, 1.0, 3.0] };
        let x = solve(&a, &[3.0, 5.0]).unwrap();
        assert!((x[0] - 0.8).abs() < 1e-12 && (x[1] - 1.4).abs() < 1e-12);
    }

    #[test]
    fn singular_is_none() {
        let a = Matrix { rows: 2, cols: 2, data: vec![1.0, 2.0, 2.0, 4.0] };
        assert!(solve(&a, &[1.0, 2.0]).is_none());
    }

    #[test]
    fn eigen_of_diagonalisable() {
        let a = Matrix { rows: 2, cols: 2, data: vec![2.0, 1.0, 1.0, 2.0] };
        let (vals, vecs) = sym_eigen(&a);
        assert!((vals[0] - 1.0).abs() < 1e-10 && (vals[1] - 3.0).abs() < 1e-10);
        assert!((vecs.get(0, 0).abs() - vecs.get(1, 0).abs()).abs() < 1e-10);
    }

    #[test]
    fn lm_fits_line() {
        let xs = [0.0, 1.0, 2.0, 3.0, 4.0];
        let res = levenberg_marquardt(&[0.0, 0.0], |p| xs.iter().map(|&x| p[0] * x + p[1] - (2.0 * x + 1.0)).collect(), LmOptions::default());
        assert!((res.params[0] - 2.0).abs() < 1e-6 && (res.params[1] - 1.0).abs() < 1e-6);
    }
}
