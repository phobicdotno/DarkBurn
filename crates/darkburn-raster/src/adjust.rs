//! Image adjustments: brightness, contrast, gamma, enhance (unsharp mask) with denoise, negative.

use serde::{Deserialize, Serialize};

use crate::gray::{GrayF, snap_endpoints};

/// Per-image adjustments, applied on the scan grid before the image mode.
///
/// All tone operations work on sRGB-encoded (perceptual) values, as people expect from
/// photo editors. Pure black and pure white are kept exact at neutral settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    /// `-100..=100`; added as a percentage of full scale.
    pub brightness: f32,
    /// `-100..=100`; `0` is neutral, `100` is a hard threshold at mid grey, `-100` flat grey.
    pub contrast: f32,
    /// `0.1..=10`; values above 1 lighten mid-tones (`v^(1/gamma)`).
    pub gamma: f32,
    /// Unsharp-mask strength, `0` = off, `1` = 100 %.
    pub enhance_amount: f32,
    /// Unsharp-mask blur radius (Gaussian sigma) in grid pixels.
    pub enhance_radius: f32,
    /// Detail smaller than this (`0..=1`, fraction of full scale) is smoothed instead of
    /// sharpened, which removes grain and JPEG noise.
    pub enhance_denoise: f32,
    /// Inverts the image after the other adjustments.
    pub negative: bool,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self { brightness: 0.0, contrast: 0.0, gamma: 1.0, enhance_amount: 0.0, enhance_radius: 1.0, enhance_denoise: 0.0, negative: false }
    }
}

impl Adjustments {
    /// True when applying would change nothing.
    pub fn is_neutral(&self) -> bool {
        self.brightness == 0.0
            && self.contrast == 0.0
            && (self.gamma - 1.0).abs() < 1e-6
            && (self.enhance_amount == 0.0 && self.enhance_denoise == 0.0)
            && !self.negative
    }

    /// Applies the adjustments in place: enhance and denoise, brightness, contrast, gamma,
    /// then negative.
    pub fn apply(&self, img: &mut GrayF) {
        if (self.enhance_amount > 0.0 || self.enhance_denoise > 0.0) && self.enhance_radius > 0.0 {
            self.enhance(img);
        }
        let b = self.brightness.clamp(-100.0, 100.0) / 100.0;
        // tan maps -100..100 onto a slope of 0..inf with 0 -> 1.
        let c = self.contrast.clamp(-100.0, 99.9) / 100.0;
        let slope = ((c + 1.0) * std::f32::consts::FRAC_PI_4).tan();
        let inv_gamma = 1.0 / self.gamma.clamp(0.1, 10.0);
        let gamma_on = (inv_gamma - 1.0).abs() > 1e-6;
        let tone_on = b != 0.0 || c != 0.0 || gamma_on;
        if !tone_on && !self.negative {
            return;
        }
        for v in &mut img.data {
            let mut x = *v;
            if tone_on {
                x += b;
                x = (x - 0.5) * slope + 0.5;
                x = x.clamp(0.0, 1.0);
                if gamma_on {
                    x = x.powf(inv_gamma);
                }
                x = snap_endpoints(x);
            }
            if self.negative {
                x = 1.0 - x;
            }
            *v = x;
        }
    }

    fn enhance(&self, img: &mut GrayF) {
        let blurred = gaussian_blur(img, self.enhance_radius);
        let amount = self.enhance_amount.max(0.0);
        let denoise = self.enhance_denoise.clamp(0.0, 1.0);
        for (v, &b) in img.data.iter_mut().zip(&blurred.data) {
            let d = *v - b;
            let t = if denoise > 0.0 { (d.abs() / denoise - 1.0).clamp(0.0, 1.0) } else { 1.0 };
            *v = snap_endpoints((b + d * t * (1.0 + amount)).clamp(0.0, 1.0));
        }
    }
}

/// Separable Gaussian blur with standard deviation `sigma` pixels, edges clamped.
pub fn gaussian_blur(img: &GrayF, sigma: f32) -> GrayF {
    if sigma <= 0.0 || img.data.is_empty() {
        return img.clone();
    }
    let r = (sigma * 3.0).ceil() as i64;
    let mut kernel: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);
    let (w, h) = (img.width as i64, img.height as i64);
    let mut tmp = vec![0.0f32; img.data.len()];
    for y in 0..h {
        let row = &img.data[(y * w) as usize..((y + 1) * w) as usize];
        let out = &mut tmp[(y * w) as usize..((y + 1) * w) as usize];
        for (x, o) in out.iter_mut().enumerate() {
            let x = x as i64;
            let mut acc = 0.0;
            for (k, &kw) in kernel.iter().enumerate() {
                let sx = (x + k as i64 - r).clamp(0, w - 1);
                acc += row[sx as usize] * kw;
            }
            *o = acc;
        }
    }
    let mut out = GrayF::new(img.width, img.height, 0.0);
    for y in 0..h {
        for (k, &kw) in kernel.iter().enumerate() {
            let sy = (y + k as i64 - r).clamp(0, h - 1);
            let src = &tmp[(sy * w) as usize..((sy + 1) * w) as usize];
            let dst = &mut out.data[(y * w) as usize..((y + 1) * w) as usize];
            for (d, &s) in dst.iter_mut().zip(src) {
                *d += s * kw;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> GrayF {
        GrayF::from_fn(256, 1, |x, _| x as f32 / 255.0)
    }

    #[test]
    fn neutral_changes_nothing() {
        let mut g = ramp();
        let a = Adjustments::default();
        assert!(a.is_neutral());
        a.apply(&mut g);
        assert_eq!(g, ramp());
    }

    #[test]
    fn brightness_contrast_gamma() {
        let mut g = GrayF::new(1, 1, 0.5);
        Adjustments { brightness: 20.0, ..Default::default() }.apply(&mut g);
        assert!((g.data[0] - 0.7).abs() < 1e-6);

        let mut g = GrayF::from_fn(2, 1, |x, _| if x == 0 { 0.4 } else { 0.6 });
        Adjustments { contrast: 50.0, ..Default::default() }.apply(&mut g);
        assert!(g.data[0] < 0.4 && g.data[1] > 0.6);
        Adjustments { contrast: 100.0, ..Default::default() }.apply(&mut g);
        assert_eq!(g.data, vec![0.0, 1.0]);

        let mut g = GrayF::new(1, 1, 0.25);
        Adjustments { gamma: 2.0, ..Default::default() }.apply(&mut g);
        assert!((g.data[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn endpoints_survive_gamma() {
        let mut g = GrayF::from_fn(2, 1, |x, _| x as f32);
        Adjustments { gamma: 0.3, ..Default::default() }.apply(&mut g);
        assert_eq!(g.data, vec![0.0, 1.0]);
    }

    #[test]
    fn negative_inverts() {
        let mut g = ramp();
        Adjustments { negative: true, ..Default::default() }.apply(&mut g);
        assert_eq!(g.data[0], 1.0);
        assert_eq!(g.data[255], 0.0);
    }

    #[test]
    fn unsharp_increases_edge_contrast() {
        let mut g = GrayF::from_fn(20, 1, |x, _| if x < 10 { 0.3 } else { 0.7 });
        Adjustments { enhance_amount: 1.0, enhance_radius: 1.5, ..Default::default() }.apply(&mut g);
        assert!(g.get(9, 0) < 0.3);
        assert!(g.get(10, 0) > 0.7);
        // Far from the edge nothing changes.
        assert!((g.get(0, 0) - 0.3).abs() < 1e-5);
    }

    #[test]
    fn denoise_smooths_small_noise_keeps_edges() {
        let noisy = GrayF::from_fn(40, 1, |x, _| {
            let base = if x < 20 { 0.2 } else { 0.8 };
            base + if x % 2 == 0 { 0.02 } else { -0.02 }
        });
        let mut g = noisy.clone();
        Adjustments { enhance_amount: 0.5, enhance_radius: 1.0, enhance_denoise: 0.05, ..Default::default() }.apply(&mut g);
        let var = |img: &GrayF| (2..15).map(|x| (img.get(x, 0) - 0.2).abs()).fold(0.0f32, f32::max);
        assert!(var(&g) < var(&noisy) * 0.5);
        assert!(g.get(19, 0) < 0.25 && g.get(20, 0) > 0.75);
    }

    #[test]
    fn blur_preserves_mean() {
        let g = GrayF::from_fn(30, 30, |x, y| ((x * 7 + y * 3) % 11) as f32 / 10.0);
        let b = gaussian_blur(&g, 2.0);
        let mean = |i: &GrayF| i.data.iter().sum::<f32>() / i.data.len() as f32;
        assert!((mean(&g) - mean(&b)).abs() < 0.02);
    }

    #[test]
    fn serde_round_trip() {
        let a = Adjustments { brightness: 5.0, negative: true, ..Default::default() };
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Adjustments>(&s).unwrap(), a);
        // Missing fields fall back to defaults.
        assert_eq!(serde_json::from_str::<Adjustments>("{}").unwrap(), Adjustments::default());
    }
}
