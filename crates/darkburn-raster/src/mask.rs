//! Image masking: clip an image with vector paths, and flatten transparency to white.

use image::{Rgb, RgbImage, RgbaImage};
use kurbo::{Affine, BezPath};

use crate::gray::{fast_to_linear, fast_to_srgb};
use crate::rasterize::to_skia_path;

/// Clips `img` (placed by `image_to_mm`) to the inside of `clip` (mm): pixels outside become
/// transparent, edge pixels partly transparent (antialiased). Returns the image unchanged in
/// size; use [`flatten_alpha`] to bake the result onto white.
pub fn apply_mask(img: &RgbaImage, image_to_mm: Affine, clip: &[BezPath], even_odd: bool) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    let Some(mut mask) = tiny_skia::Mask::new(w, h) else {
        return out;
    };
    if let Some(path) = to_skia_path(clip, image_to_mm.inverse()) {
        let rule = if even_odd { tiny_skia::FillRule::EvenOdd } else { tiny_skia::FillRule::Winding };
        mask.fill_path(&path, rule, true, tiny_skia::Transform::identity());
    }
    for (p, &m) in out.pixels_mut().zip(mask.data()) {
        p.0[3] = ((u16::from(p.0[3]) * u16::from(m) + 127) / 255) as u8;
    }
    out
}

/// Composites an image with transparency over white, in linear light.
pub fn flatten_alpha(img: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let [r, g, b, a] = img.get_pixel(x, y).0;
        if a == 255 {
            return Rgb([r, g, b]);
        }
        if a == 0 {
            return Rgb([255, 255, 255]);
        }
        let af = f32::from(a) / 255.0;
        let ch = |c: u8| (fast_to_srgb(fast_to_linear(f32::from(c) / 255.0) * af + (1.0 - af)) * 255.0).round() as u8;
        Rgb([ch(r), ch(g), ch(b)])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement;
    use image::Rgba;
    use kurbo::{Point, Rect, Shape, Size};

    #[test]
    fn mask_clips_outside() {
        let img = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let t = placement(20, 20, Point::ZERO, Size::new(20.0, 20.0));
        // Keep the left half (x < 10 mm).
        let clip = Rect::new(0.0, 0.0, 10.0, 20.0).to_path(0.01);
        let out = apply_mask(&img, t, &[clip], false);
        assert_eq!(out.get_pixel(2, 5).0[3], 255);
        assert_eq!(out.get_pixel(15, 5).0[3], 0);
        let flat = flatten_alpha(&out);
        assert_eq!(flat.get_pixel(2, 5).0, [0, 0, 0]);
        assert_eq!(flat.get_pixel(15, 5).0, [255, 255, 255]);
    }

    #[test]
    fn half_alpha_flattens_in_linear_light() {
        let img = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 128]));
        let v = flatten_alpha(&img).get_pixel(0, 0).0[0];
        assert!((186..=189).contains(&v), "{v}");
    }
}
