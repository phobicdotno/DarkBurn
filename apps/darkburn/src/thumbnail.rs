//! The project thumbnail saved inside `.dbrn` files: shape outlines in their layer colours on
//! white, fitted into a square PNG.

use darkburn_core::Document;
use darkburn_core::kurbo::{Affine, PathEl};
use darkburn_core::layer::layer_colour;

/// Thumbnail edge in pixels.
pub const SIZE: u32 = 256;
const MARGIN: f64 = 8.0;

/// The document's outlines as PNG bytes; None for an empty document.
pub fn render(doc: &Document) -> Option<Vec<u8>> {
    let bb = doc.bbox()?;
    let mut pixmap = tiny_skia::Pixmap::new(SIZE, SIZE)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let span = bb.width().max(bb.height()).max(1e-6);
    let k = (SIZE as f64 - 2.0 * MARGIN) / span;
    // Document Y is up; pixels go down. Centre the drawing.
    let c = bb.center();
    let to_px = Affine::translate((SIZE as f64 / 2.0, SIZE as f64 / 2.0)) * Affine::scale_non_uniform(k, -k) * Affine::translate(-c.to_vec2());
    let stroke = tiny_skia::Stroke { width: 1.25, ..Default::default() };
    for shape in doc.iter() {
        for leaf in shape.leaves() {
            let [r, g, b] = layer_colour(leaf.layer);
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(r, g, b, 255);
            paint.anti_alias = true;
            let mut pb = tiny_skia::PathBuilder::new();
            let p = |q: darkburn_core::kurbo::Point| {
                let q = to_px * q;
                (q.x as f32, q.y as f32)
            };
            for el in leaf.outline().elements() {
                match *el {
                    PathEl::MoveTo(a) => {
                        let (x, y) = p(a);
                        pb.move_to(x, y);
                    }
                    PathEl::LineTo(a) => {
                        let (x, y) = p(a);
                        pb.line_to(x, y);
                    }
                    PathEl::QuadTo(a, b) => {
                        let ((x1, y1), (x, y)) = (p(a), p(b));
                        pb.quad_to(x1, y1, x, y);
                    }
                    PathEl::CurveTo(a, b, c) => {
                        let ((x1, y1), (x2, y2), (x, y)) = (p(a), p(b), p(c));
                        pb.cubic_to(x1, y1, x2, y2, x, y);
                    }
                    PathEl::ClosePath => pb.close(),
                }
            }
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
            }
        }
    }
    // Opaque white background: premultiplied and straight RGBA are the same.
    crate::import::rgba_to_png(SIZE, SIZE, pixmap.data().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect;

    #[test]
    fn thumbnail_draws_the_outlines() {
        assert!(render(&Document::new()).is_none());
        let mut d = Document::new();
        d.add(Shape::rect(Rect::new(0.0, 0.0, 100.0, 50.0), 0.0, 1));
        d.add(Shape::ellipse(Rect::new(10.0, 10.0, 40.0, 40.0), 0));
        let png = render(&d).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (SIZE, SIZE));
        let inked = img.pixels().filter(|p| p.0[0] < 200 || p.0[1] < 200 || p.0[2] < 200).count();
        assert!(inked > 300, "outlines are drawn: {inked}");
    }
}
