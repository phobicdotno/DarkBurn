//! Image helpers shared by the lens and camera modules.
//!
//! Pixel coordinate convention: the centre of pixel `(i, j)` is the point `(i, j)`.

use image::{ImageBuffer, Pixel};

/// Bilinear sample with edge clamping. `None` when more than half a pixel outside the image.
pub fn sample_bilinear<P: Pixel<Subpixel = u8>>(src: &ImageBuffer<P, Vec<u8>>, x: f64, y: f64) -> Option<[u8; 4]> {
    let (w, h) = (src.width() as f64, src.height() as f64);
    if src.width() == 0 || src.height() == 0 || !(x.is_finite() && y.is_finite()) || x < -0.5 || y < -0.5 || x > w - 0.5 || y > h - 0.5 {
        return None;
    }
    let xc = x.clamp(0.0, w - 1.0);
    let yc = y.clamp(0.0, h - 1.0);
    let x0 = xc.floor() as u32;
    let y0 = yc.floor() as u32;
    let x1 = (x0 + 1).min(src.width() - 1);
    let y1 = (y0 + 1).min(src.height() - 1);
    let fx = xc - x0 as f64;
    let fy = yc - y0 as f64;
    let n = P::CHANNEL_COUNT as usize;
    let p00 = src.get_pixel(x0, y0).channels();
    let p10 = src.get_pixel(x1, y0).channels();
    let p01 = src.get_pixel(x0, y1).channels();
    let p11 = src.get_pixel(x1, y1).channels();
    let mut out = [0u8; 4];
    for c in 0..n.min(4) {
        let top = p00[c] as f64 * (1.0 - fx) + p10[c] as f64 * fx;
        let bot = p01[c] as f64 * (1.0 - fx) + p11[c] as f64 * fx;
        out[c] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

/// Build an output image where each pixel is sampled from `map(out_x, out_y)` in the source.
pub fn remap_bilinear<P: Pixel<Subpixel = u8>>(
    src: &ImageBuffer<P, Vec<u8>>,
    out_w: u32,
    out_h: u32,
    map: impl Fn(f64, f64) -> Option<(f64, f64)>,
) -> ImageBuffer<P, Vec<u8>> {
    let mut out = ImageBuffer::<P, Vec<u8>>::new(out_w, out_h);
    let n = P::CHANNEL_COUNT as usize;
    for y in 0..out_h {
        for x in 0..out_w {
            if let Some((sx, sy)) = map(x as f64, y as f64)
                && let Some(v) = sample_bilinear(src, sx, sy)
            {
                out.put_pixel(x, y, *P::from_slice(&v[..n]));
            }
        }
    }
    out
}
