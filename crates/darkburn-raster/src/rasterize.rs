//! Vectors to bitmap ("Convert to bitmap"), via tiny-skia.

use kurbo::{Affine, BezPath, PathEl, Rect, Shape};

use crate::gray::GrayF;

/// How vectors are painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaintStyle {
    /// Fill closed shapes; `even_odd` picks the fill rule (otherwise non-zero winding).
    Fill { even_odd: bool },
    /// Stroke outlines with a pen `width_mm` wide.
    Stroke { width_mm: f64 },
}

/// Why rasterising failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RasterError {
    /// Nothing to draw, or zero-sized bounds.
    Empty,
    /// The bitmap would exceed the pixel limit.
    TooLarge { width: u64, height: u64, max_pixels: u64 },
}

impl std::fmt::Display for RasterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "nothing to convert"),
            Self::TooLarge { width, height, max_pixels } => {
                write!(f, "bitmap of {width} × {height} pixels is larger than the limit of {max_pixels} pixels")
            }
        }
    }
}

impl std::error::Error for RasterError {}

/// A rasterised bitmap and where it sits.
#[derive(Clone, Debug, PartialEq)]
pub struct RasterizedPaths {
    /// Black artwork on white, antialiased.
    pub image: GrayF,
    /// Image pixels to machine mm.
    pub image_to_mm: Affine,
}

/// Converts kurbo paths (already in the target pixel space) to one tiny-skia path.
pub(crate) fn to_skia_path(paths: &[BezPath], transform: Affine) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    for path in paths {
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    let p = transform * p;
                    pb.move_to(p.x as f32, p.y as f32);
                }
                PathEl::LineTo(p) => {
                    let p = transform * p;
                    pb.line_to(p.x as f32, p.y as f32);
                }
                PathEl::QuadTo(a, p) => {
                    let (a, p) = (transform * a, transform * p);
                    pb.quad_to(a.x as f32, a.y as f32, p.x as f32, p.y as f32);
                }
                PathEl::CurveTo(a, b, p) => {
                    let (a, b, p) = (transform * a, transform * b, transform * p);
                    pb.cubic_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32, p.x as f32, p.y as f32);
                }
                PathEl::ClosePath => pb.close(),
            }
        }
    }
    pb.finish()
}

/// Rasterises `paths` (mm) at `dpi`. The bitmap covers `bounds_mm`, or the paths' bounding
/// box (grown by half the pen width when stroking) when `None`. Fails when the bitmap would
/// have more than `max_pixels` pixels.
pub fn rasterize_paths(
    paths: &[BezPath],
    style: PaintStyle,
    dpi: f64,
    bounds_mm: Option<Rect>,
    max_pixels: u64,
) -> Result<RasterizedPaths, RasterError> {
    let px_mm = 25.4 / dpi.max(1e-6);
    let bounds = match bounds_mm {
        Some(b) => b,
        None => {
            let mut bb: Option<Rect> = None;
            for p in paths {
                if p.elements().is_empty() {
                    continue;
                }
                let b = p.bounding_box();
                bb = Some(bb.map_or(b, |a| a.union(b)));
            }
            let bb = bb.ok_or(RasterError::Empty)?;
            match style {
                PaintStyle::Stroke { width_mm } => bb.inflate(width_mm / 2.0, width_mm / 2.0),
                PaintStyle::Fill { .. } => bb,
            }
        }
    };
    let width = (bounds.width() / px_mm).ceil().max(0.0) as u64;
    let height = (bounds.height() / px_mm).ceil().max(0.0) as u64;
    if width == 0 || height == 0 {
        return Err(RasterError::Empty);
    }
    if width * height > max_pixels || width > u64::from(u32::MAX) || height > u64::from(u32::MAX) {
        return Err(RasterError::TooLarge { width, height, max_pixels });
    }
    let (w, h) = (width as u32, height as u32);
    let image_to_mm = Affine::new([px_mm, 0.0, 0.0, -px_mm, bounds.x0, bounds.y0 + f64::from(h) * px_mm]);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or(RasterError::TooLarge { width, height, max_pixels })?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let Some(path) = to_skia_path(paths, image_to_mm.inverse()) else {
        return Ok(RasterizedPaths { image: GrayF::new(w, h, 1.0), image_to_mm });
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color(tiny_skia::Color::BLACK);
    paint.anti_alias = true;
    match style {
        PaintStyle::Fill { even_odd } => {
            let rule = if even_odd { tiny_skia::FillRule::EvenOdd } else { tiny_skia::FillRule::Winding };
            pixmap.fill_path(&path, &paint, rule, tiny_skia::Transform::identity(), None);
        }
        PaintStyle::Stroke { width_mm } => {
            let stroke = tiny_skia::Stroke {
                width: (width_mm / px_mm) as f32,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }
    let data = pixmap.pixels().iter().map(|p| f32::from(p.red()) / 255.0).collect();
    Ok(RasterizedPaths { image: GrayF { width: w, height: h, data, clear: Vec::new() }, image_to_mm })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Circle, Point};

    #[test]
    fn fills_a_square_at_dpi() {
        let sq = Rect::new(0.0, 0.0, 25.4, 12.7).to_path(0.01);
        let r = rasterize_paths(&[sq], PaintStyle::Fill { even_odd: false }, 100.0, None, 1 << 30).unwrap();
        assert_eq!((r.image.width, r.image.height), (100, 50));
        assert!(r.image.data.iter().all(|&v| v == 0.0));
        let corner = r.image_to_mm * Point::new(0.0, 50.0);
        assert!(corner.distance(Point::ZERO) < 1e-9);
    }

    #[test]
    fn even_odd_makes_holes_and_y_is_up() {
        let outer = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
        let inner = Rect::new(2.0, 2.0, 4.0, 4.0).to_path(0.01);
        let r = rasterize_paths(&[outer, inner], PaintStyle::Fill { even_odd: true }, 25.4, None, 1 << 30).unwrap();
        // 1 px per mm. The hole is near the bottom-left in mm, so near the bottom-left in
        // image rows too (rows run top-down).
        assert_eq!(r.image.get(3, 6), 1.0);
        assert_eq!(r.image.get(3, 2), 0.0);
    }

    #[test]
    fn stroke_width() {
        let c = Circle::new((10.0, 10.0), 5.0).to_path(0.01);
        let r = rasterize_paths(&[c], PaintStyle::Stroke { width_mm: 1.0 }, 254.0, None, 1 << 30).unwrap();
        let ink: f32 = r.image.data.iter().map(|v| 1.0 - v).sum::<f32>() * 0.01;
        let expected = 2.0 * std::f32::consts::PI * 5.0;
        assert!((ink - expected).abs() < 0.5, "{ink}");
    }

    #[test]
    fn limits() {
        let sq = Rect::new(0.0, 0.0, 1000.0, 1000.0).to_path(0.01);
        assert!(matches!(rasterize_paths(&[sq], PaintStyle::Fill { even_odd: false }, 1000.0, None, 1 << 20), Err(RasterError::TooLarge { .. })));
        assert_eq!(rasterize_paths(&[], PaintStyle::Fill { even_odd: false }, 100.0, None, 1 << 20), Err(RasterError::Empty));
    }
}
