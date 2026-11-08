//! Grey images, power maps and sRGB transfer functions.

use std::sync::OnceLock;

use image::{DynamicImage, GrayImage, Luma};

/// Exact sRGB decode (sRGB-encoded value to linear light), both in `0..=1`.
pub fn srgb_to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Exact sRGB encode (linear light to sRGB-encoded value), both in `0..=1`.
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v >= 1.0 {
        1.0
    } else if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

const LUT_SIZE: usize = 4096;

struct Luts {
    to_linear: Vec<f32>,
    to_srgb: Vec<f32>,
    u8_to_linear: [f32; 256],
}

fn luts() -> &'static Luts {
    static LUTS: OnceLock<Luts> = OnceLock::new();
    LUTS.get_or_init(|| {
        let n = LUT_SIZE as f32;
        let to_linear = (0..=LUT_SIZE).map(|i| srgb_to_linear(i as f32 / n)).collect();
        let to_srgb = (0..=LUT_SIZE).map(|i| linear_to_srgb(i as f32 / n)).collect();
        let mut u8_to_linear = [0.0; 256];
        for (i, v) in u8_to_linear.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        Luts { to_linear, to_srgb, u8_to_linear }
    })
}

#[inline]
fn lut_lookup(table: &[f32], v: f32) -> f32 {
    if v >= 1.0 {
        return table[LUT_SIZE];
    }
    let x = v.max(0.0) * LUT_SIZE as f32;
    let i = (x as usize).min(LUT_SIZE - 1);
    let f = x - i as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

/// Fast table-based [`srgb_to_linear`]; exact at 0 and 1.
#[inline]
pub(crate) fn fast_to_linear(v: f32) -> f32 {
    lut_lookup(&luts().to_linear, v)
}

/// Fast table-based [`linear_to_srgb`]; exact at 0 and 1.
#[inline]
pub(crate) fn fast_to_srgb(v: f32) -> f32 {
    lut_lookup(&luts().to_srgb, v)
}

/// A grey image with `f32` samples in `0..=1`, sRGB-encoded (perceptual), 0 = black, 1 = white.
/// Rows run top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct GrayF {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl GrayF {
    /// A `width × height` image filled with `value`.
    pub fn new(width: u32, height: u32, value: f32) -> Self {
        Self { width, height, data: vec![value; width as usize * height as usize] }
    }

    /// Builds an image from a function of `(x, y)`.
    pub fn from_fn(width: u32, height: u32, f: impl Fn(u32, u32) -> f32) -> Self {
        let mut data = Vec::with_capacity(width as usize * height as usize);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self { width, height, data }
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32) -> f32 {
        self.data[y as usize * self.width as usize + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, v: f32) {
        let w = self.width as usize;
        self.data[y as usize * w + x as usize] = v;
    }

    /// Converts any decoded image to grey.
    ///
    /// Luminance is computed in linear light (Rec. 709 weights), transparent pixels are
    /// composited over white in linear light, and the result is sRGB-encoded again. Grey
    /// opaque pixels pass through exactly, so pure black and pure white stay exact.
    pub fn from_dynamic(img: &DynamicImage) -> Self {
        let lin = &luts().u8_to_linear;
        let (width, height) = (img.width(), img.height());
        let high_depth =
            !matches!(img, DynamicImage::ImageLuma8(_) | DynamicImage::ImageLumaA8(_) | DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_));
        let data = match img {
            DynamicImage::ImageLuma8(g) => g.as_raw().iter().map(|&v| f32::from(v) / 255.0).collect(),
            _ if high_depth => {
                let rgba = img.to_rgba32f();
                rgba.pixels()
                    .map(|p| {
                        let [r, g, b, a] = p.0;
                        let y = 0.2126 * srgb_to_linear(r) + 0.7152 * srgb_to_linear(g) + 0.0722 * srgb_to_linear(b);
                        let a = a.clamp(0.0, 1.0);
                        if a >= 1.0 && r == g && g == b { r.clamp(0.0, 1.0) } else { linear_to_srgb(a * y + (1.0 - a)) }
                    })
                    .collect()
            }
            _ => {
                let rgba = img.to_rgba8();
                rgba.pixels()
                    .map(|p| {
                        let [r, g, b, a] = p.0;
                        if a == 255 && r == g && g == b {
                            return f32::from(r) / 255.0;
                        }
                        if a == 0 {
                            return 1.0;
                        }
                        let y = 0.2126 * lin[r as usize] + 0.7152 * lin[g as usize] + 0.0722 * lin[b as usize];
                        let a = f32::from(a) / 255.0;
                        snap_endpoints(linear_to_srgb(a * y + (1.0 - a)))
                    })
                    .collect()
            }
        };
        Self { width, height, data }
    }

    /// From an 8-bit grey image.
    pub fn from_luma8(img: &GrayImage) -> Self {
        Self { width: img.width(), height: img.height(), data: img.as_raw().iter().map(|&v| f32::from(v) / 255.0).collect() }
    }

    /// To an 8-bit grey image (rounded).
    pub fn to_luma8(&self) -> GrayImage {
        GrayImage::from_fn(self.width, self.height, |x, y| Luma([(self.get(x, y).clamp(0.0, 1.0) * 255.0).round() as u8]))
    }

    /// The same image in linear light.
    pub fn to_linear(&self) -> Vec<f32> {
        self.data.iter().map(|&v| fast_to_linear(v)).collect()
    }
}

/// Values within half an 8-bit step of 0 or 1 become exactly 0 or 1, so white never burns
/// and black is always full power.
#[inline]
pub(crate) fn snap_endpoints(v: f32) -> f32 {
    const EPS: f32 = 0.5 / 255.0;
    if v < EPS {
        0.0
    } else if v > 1.0 - EPS {
        1.0
    } else {
        v
    }
}

/// A power map on a scan grid: `0` = laser off, `255` = full power. Rows run top to bottom.
///
/// `binary` maps hold only `0` and `255` (dithered or thresholded modes); others hold power
/// levels for grayscale engraving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerMap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub binary: bool,
}

impl PowerMap {
    pub fn new(width: u32, height: u32, binary: bool) -> Self {
        Self { width, height, data: vec![0; width as usize * height as usize], binary }
    }

    #[inline]
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.data[y as usize * self.width as usize + x as usize]
    }

    /// One row of the map.
    pub fn row(&self, y: u32) -> &[u8] {
        let w = self.width as usize;
        &self.data[y as usize * w..(y as usize + 1) * w]
    }

    /// Number of non-zero pixels.
    pub fn on_count(&self) -> usize {
        self.data.iter().filter(|&&v| v != 0).count()
    }

    /// What the burn looks like: white where off, black at full power.
    pub fn to_preview(&self) -> GrayImage {
        GrayImage::from_raw(self.width, self.height, self.data.iter().map(|&v| 255 - v).collect()).expect("size matches")
    }

    /// Reads a preview-style image back (dark = power).
    pub fn from_preview(img: &GrayImage, binary: bool) -> Self {
        Self { width: img.width(), height: img.height(), data: img.as_raw().iter().map(|&v| 255 - v).collect(), binary }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn srgb_round_trip_and_endpoints() {
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
            assert!((fast_to_srgb(fast_to_linear(v)) - v).abs() < 2e-3);
        }
        assert_eq!(fast_to_linear(0.0), 0.0);
        assert_eq!(fast_to_linear(1.0), 1.0);
        assert_eq!(fast_to_srgb(1.0), 1.0);
        // Mid grey is about 21.4 % linear light.
        assert!((srgb_to_linear(0.5) - 0.214).abs() < 1e-3);
    }

    #[test]
    fn transparent_becomes_white_and_grey_is_exact() {
        let mut img = RgbaImage::new(3, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        img.put_pixel(2, 0, Rgba([0, 0, 0, 128]));
        let g = GrayF::from_dynamic(&DynamicImage::ImageRgba8(img));
        assert_eq!(g.data[0], 1.0);
        assert_eq!(g.data[1], 0.0);
        // Half-transparent black over white is half linear light, about 0.735 sRGB.
        assert!((g.data[2] - 0.737).abs() < 0.01, "{}", g.data[2]);
    }

    #[test]
    fn colour_luminance_in_linear_light() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, Rgba([0, 255, 0, 255]));
        let g = GrayF::from_dynamic(&DynamicImage::ImageRgba8(img));
        assert!((g.data[0] - linear_to_srgb(0.7152)).abs() < 1e-4);
    }

    #[test]
    fn power_map_preview_round_trip() {
        let mut m = PowerMap::new(2, 1, false);
        m.data = vec![0, 200];
        let p = m.to_preview();
        assert_eq!(PowerMap::from_preview(&p, false), m);
        assert_eq!(m.on_count(), 1);
    }
}
