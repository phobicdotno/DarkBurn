//! Drawing the document on the canvas: outlines in their layer colours, filled shapes, images,
//! the selection and the layer flash.
//!
//! Curves are flattened in screen space, so the tolerance (fine for the smooth view style,
//! coarse for the fast one) follows the zoom by itself. Fills use the non-zero rule and are cut
//! into trapezoids between the outline's vertex heights, which is exact for concave shapes and
//! shapes with holes (egui itself only fills convex polygons).

use std::collections::{HashMap, HashSet};

use darkburn_core::kurbo::{self, Affine, BezPath, Point};
use darkburn_core::{Document, Shape, ShapeId, ShapeKind};
use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Context, Painter, Pos2, Rect, Stroke, TextureHandle, pos2};

/// Document millimetres (Y up) to screen points and back, for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    /// Screen position of the page's top-left corner.
    pub origin: Pos2,
    /// Screen points per millimetre.
    pub zoom: f64,
    /// Page height in mm (document Y = page_h - page v).
    pub page_h: f64,
}

impl Xf {
    pub fn new(canvas: Rect, view: &crate::view::View, page_h: f32) -> Xf {
        Xf { origin: canvas.min + view.offset, zoom: view.zoom as f64, page_h: page_h as f64 }
    }

    pub fn to_screen(self, p: Point) -> Pos2 {
        pos2((self.origin.x as f64 + p.x * self.zoom) as f32, (self.origin.y as f64 + (self.page_h - p.y) * self.zoom) as f32)
    }

    pub fn to_doc(self, s: Pos2) -> Point {
        Point::new((s.x - self.origin.x) as f64 / self.zoom, self.page_h - (s.y - self.origin.y) as f64 / self.zoom)
    }

    /// The document-to-screen transform.
    pub fn affine(self) -> Affine {
        let z = self.zoom;
        Affine::new([z, 0.0, 0.0, -z, self.origin.x as f64, self.origin.y as f64 + self.page_h * z])
    }

    /// Millimetres in `px` screen points.
    pub fn mm(self, px: f32) -> f64 {
        px as f64 / self.zoom
    }

    /// Screen rectangle of document rectangle `r`.
    pub fn rect(self, r: kurbo::Rect) -> Rect {
        Rect::from_two_pos(self.to_screen(Point::new(r.x0, r.y0)), self.to_screen(Point::new(r.x1, r.y1)))
    }
}

/// Flattening tolerance in screen points: smooth or coarse.
pub fn tolerance(smooth: bool) -> f64 {
    if smooth { 0.2 } else { 1.5 }
}

/// A path (document coordinates) as screen polylines, with whether each is closed.
pub fn screen_polylines(path: &BezPath, xf: &Xf, smooth: bool) -> Vec<(Vec<Pos2>, bool)> {
    let mut p = path.clone();
    p.apply_affine(xf.affine());
    darkburn_geom::path::flatten_to_polylines(&p, tolerance(smooth))
        .into_iter()
        .map(|pl| {
            let mut pts: Vec<Pos2> = pl.points.iter().map(|q| pos2(q.x as f32, q.y as f32)).collect();
            // A path that ends where it began is closed even without a close command.
            let ends_meet = pts.len() > 2 && (pts[0] - pts[pts.len() - 1]).length() < 0.01;
            if ends_meet {
                pts.pop();
            }
            (pts, pl.closed || ends_meet)
        })
        .collect()
}

/// A trapezoid with horizontal top and bottom: top-left, top-right, bottom-right, bottom-left.
pub type Trapezoid = [Pos2; 4];

struct Edge {
    y0: f32,
    y1: f32,
    x0: f32,
    dxdy: f32,
    dir: i32,
}

impl Edge {
    fn x(&self, y: f32) -> f32 {
        self.x0 + (y - self.y0) * self.dxdy
    }
}

/// The inside (non-zero winding) of the closed polygons `polys`, between heights `y_min` and
/// `y_max`, as trapezoids.
pub fn fill_trapezoids(polys: &[Vec<Pos2>], y_min: f32, y_max: f32) -> Vec<Trapezoid> {
    let mut edges = Vec::new();
    let mut ys = vec![y_min, y_max];
    for poly in polys {
        let n = poly.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if !(a.y.is_finite() && b.y.is_finite() && a.x.is_finite() && b.x.is_finite()) || a.y == b.y {
                continue;
            }
            let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            edges.push(Edge { y0: lo.y, y1: hi.y, x0: lo.x, dxdy: (hi.x - lo.x) / (hi.y - lo.y), dir });
            ys.push(a.y);
        }
    }
    let mut out = Vec::new();
    if edges.is_empty() || !(y_max > y_min) {
        return out;
    }
    ys.retain(|y| *y >= y_min && *y <= y_max);
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    edges.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    let mut next = 0;
    let mut active: Vec<usize> = Vec::new();
    for w in ys.windows(2) {
        let (ya, yb) = (w[0], w[1]);
        while next < edges.len() && edges[next].y0 <= ya {
            active.push(next);
            next += 1;
        }
        active.retain(|i| edges[*i].y1 > ya);
        let spanning: Vec<&Edge> = active.iter().map(|i| &edges[*i]).filter(|e| e.y0 <= ya && e.y1 >= yb).collect();
        slab(&spanning, ya, yb, 0, &mut out);
    }
    out
}

/// One horizontal slab: every edge in `edges` spans it. Edges that cross inside it (a shape
/// crossing itself) split it in two, a few times over.
fn slab(edges: &[&Edge], ya: f32, yb: f32, depth: u32, out: &mut Vec<Trapezoid>) {
    if yb - ya < 1e-4 || edges.len() < 2 {
        return;
    }
    let ym = (ya + yb) * 0.5;
    let mut order: Vec<(f32, f32, f32, i32)> = edges.iter().map(|e| (e.x(ym), e.x(ya), e.x(yb), e.dir)).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0));
    let crossed = order.windows(2).any(|w| w[1].1 < w[0].1 - 0.01 || w[1].2 < w[0].2 - 0.01);
    if crossed && depth < 8 && yb - ya > 0.25 {
        slab(edges, ya, ym, depth + 1, out);
        slab(edges, ym, yb, depth + 1, out);
        return;
    }
    let mut winding = 0;
    let mut start = None;
    for (_, xa, xb, dir) in order {
        let before = winding;
        winding += dir;
        if before == 0 && winding != 0 {
            start = Some((xa, xb));
        } else if before != 0
            && winding == 0
            && let Some((sa, sb)) = start.take()
        {
            out.push([pos2(sa, ya), pos2(xa, ya), pos2(xb, yb), pos2(sb, yb)]);
        }
    }
}

/// Add trapezoids to a coloured mesh.
pub fn add_trapezoids(mesh: &mut Mesh, traps: &[Trapezoid], colour: Color32) {
    for t in traps {
        let base = mesh.vertices.len() as u32;
        for p in t {
            mesh.colored_vertex(*p, colour);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
}

/// What to draw this frame.
pub struct Scene<'a> {
    pub doc: &'a Document,
    pub selected: &'a [ShapeId],
    pub filled: bool,
    pub smooth: bool,
    /// The canvas is dark (layer 00, black, is drawn light).
    pub dark: bool,
    pub highlight: Color32,
    /// The work area (document mm): shapes reaching outside it are drawn faintly.
    pub page: kurbo::Rect,
    /// A layer flashing, and whether this is a lit moment of the blink.
    pub flash: Option<(usize, bool)>,
}

/// How strongly faint shapes (non-output layers, outside the work area) are drawn.
const FAINT: f32 = 0.35;

/// The colour shapes on layer `i` are drawn in.
pub fn ink(i: usize, dark: bool) -> Color32 {
    let c = crate::palette::layer_colour(i);
    let lum = 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
    if dark && lum < 60.0 { Color32::from_gray(220) } else { c }
}

/// Draw the document.
pub fn paint(painter: &Painter, ctx: &Context, xf: &Xf, scene: &Scene) {
    let clip = painter.clip_rect();
    let doc = scene.doc;
    let selected: HashSet<ShapeId> = scene.selected.iter().copied().collect();
    let mut fills = Mesh::default();
    let mut lines: Vec<egui::Shape> = Vec::new();
    let mut overlay: Vec<egui::Shape> = Vec::new();
    textures_prune(ctx, doc);
    for top in doc.iter() {
        let is_sel = selected.contains(&top.id);
        let outside = !scene.page.inflate(1e-6, 1e-6).contains_rect(top.bbox());
        for leaf in top.leaves() {
            let layer = doc.layer(leaf.layer);
            let bbox = leaf.bbox();
            if !xf.rect(bbox).expand(4.0).intersects(clip) {
                continue;
            }
            let mut alpha = if outside || !layer.output { FAINT } else { 1.0 };
            if let ShapeKind::Image(img) = &leaf.kind {
                if !layer.show {
                    alpha *= 0.3;
                }
                paint_image(painter, ctx, xf, doc, &leaf, img, alpha);
                if is_sel {
                    for (pts, _) in screen_polylines(&leaf.outline(), xf, scene.smooth) {
                        let mut pts = pts;
                        pts.push(pts[0]);
                        egui::Shape::dashed_line_many(&pts, Stroke::new(1.5, scene.highlight), 5.0, 4.0, &mut overlay);
                    }
                }
                continue;
            }
            if !layer.show {
                continue;
            }
            let colour = ink(leaf.layer, scene.dark).gamma_multiply(alpha);
            let polys = screen_polylines(&leaf.outline(), xf, scene.smooth);
            if scene.filled && doc.is_filled(&leaf) {
                let closed: Vec<Vec<Pos2>> = polys.iter().map(|(p, _)| p.clone()).collect();
                let traps = fill_trapezoids(&closed, clip.top() - 1.0, clip.bottom() + 1.0);
                add_trapezoids(&mut fills, &traps, colour.gamma_multiply(0.75));
            }
            let flashing = scene.flash.is_some_and(|(l, on)| on && l == leaf.layer);
            for (mut pts, closed) in polys {
                if pts.len() < 2 {
                    continue;
                }
                if closed {
                    pts.push(pts[0]);
                }
                if flashing {
                    overlay.push(egui::Shape::line(pts.clone(), Stroke::new(4.0, scene.highlight.gamma_multiply(0.8))));
                }
                if is_sel {
                    overlay.push(egui::Shape::line(pts.clone(), Stroke::new(3.5, scene.highlight.gamma_multiply(0.25))));
                    egui::Shape::dashed_line_many(&pts, Stroke::new(1.5, colour), 6.0, 4.0, &mut overlay);
                } else {
                    lines.push(egui::Shape::line(pts, Stroke::new(1.0, colour)));
                }
            }
        }
    }
    if !fills.is_empty() {
        painter.add(egui::Shape::mesh(fills));
    }
    painter.extend(lines);
    painter.extend(overlay);
}

/// The texture cache: image key to texture (None when the bytes did not decode).
type Textures = HashMap<String, Option<TextureHandle>>;

fn textures_id() -> egui::Id {
    egui::Id::unique("darkburn_canvas_textures")
}

/// Forget textures of images no longer in the document.
fn textures_prune(ctx: &Context, doc: &Document) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(textures_id()).retain(|k, _| doc.images.contains_key(k)));
}

/// The texture of image `key`, decoded on first use.
fn texture(ctx: &Context, doc: &Document, key: &str) -> Option<TextureHandle> {
    if let Some(t) = ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(textures_id()).get(key).cloned()) {
        return t;
    }
    let decoded = doc.images.get(key).and_then(|data| {
        let img = image::load_from_memory(&data.bytes).ok()?;
        let max = ctx.input(|i| i.max_texture_side).min(4096) as u32;
        let img = if img.width() > max || img.height() > max { img.resize(max, max, image::imageops::FilterType::Triangle) } else { img };
        let rgba = img.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let colour = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        Some(ctx.load_texture(format!("db_image_{key}"), colour, egui::TextureOptions::LINEAR))
    });
    if decoded.is_none() {
        log::warn!("image {key} could not be decoded for display");
    }
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(textures_id()).insert(key.to_string(), decoded.clone()));
    decoded
}

/// Draw an image shape as a textured quad following its transform (rotation and shear too).
fn paint_image(painter: &Painter, ctx: &Context, xf: &Xf, doc: &Document, leaf: &Shape, img: &darkburn_core::ImageShape, alpha: f32) {
    let (w, h) = (img.width / 2.0, img.height / 2.0);
    let corners = [(-w, h, 0.0, 0.0), (w, h, 1.0, 0.0), (w, -h, 1.0, 1.0), (-w, -h, 0.0, 1.0)];
    let Some(tex) = crate::image_render::texture(ctx, doc, leaf, img).or_else(|| texture(ctx, doc, &img.image)) else {
        // Undecodable: a grey box keeps the shape visible.
        let pts: Vec<Pos2> = corners.iter().map(|(x, y, _, _)| xf.to_screen(leaf.transform * Point::new(*x, *y))).collect();
        painter.add(egui::Shape::closed_line(pts, Stroke::new(1.0, Color32::GRAY.gamma_multiply(alpha))));
        return;
    };
    let mut mesh = Mesh::with_texture(tex.id());
    let tint = Color32::WHITE.gamma_multiply(alpha);
    for (x, y, u, v) in corners {
        mesh.vertices.push(Vertex { pos: xf.to_screen(leaf.transform * Point::new(x, y)), uv: pos2(u, v), color: tint });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::kurbo::Rect as KRect;
    use egui::vec2;

    fn area(traps: &[Trapezoid]) -> f32 {
        traps.iter().map(|t| ((t[1].x - t[0].x) + (t[2].x - t[3].x)) * 0.5 * (t[3].y - t[0].y)).sum()
    }

    fn square(x: f32, y: f32, s: f32, ccw: bool) -> Vec<Pos2> {
        let mut v = vec![pos2(x, y), pos2(x + s, y), pos2(x + s, y + s), pos2(x, y + s)];
        if ccw {
            v.reverse();
        }
        v
    }

    #[test]
    fn square_with_a_hole() {
        let outer = square(0.0, 0.0, 10.0, false);
        let hole = square(3.0, 3.0, 4.0, true);
        let traps = fill_trapezoids(&[outer.clone(), hole], -100.0, 100.0);
        assert!((area(&traps) - 84.0).abs() < 1e-3, "{}", area(&traps));
        // Same direction: non-zero fills the inner square too.
        let same = square(3.0, 3.0, 4.0, false);
        assert!((area(&fill_trapezoids(&[outer, same], -100.0, 100.0)) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn concave_shape_and_clipping() {
        // A "U": 10 wide, 10 high, with a 4 wide, 6 deep notch from the top.
        let u =
            vec![pos2(0.0, 0.0), pos2(3.0, 0.0), pos2(3.0, 6.0), pos2(7.0, 6.0), pos2(7.0, 0.0), pos2(10.0, 0.0), pos2(10.0, 10.0), pos2(0.0, 10.0)];
        let traps = fill_trapezoids(std::slice::from_ref(&u), -1.0, 20.0);
        assert!((area(&traps) - 76.0).abs() < 1e-3, "{}", area(&traps));
        let half = fill_trapezoids(&[u], 5.0, 20.0);
        assert!((area(&half) - (100.0 - 24.0 - 5.0 * 10.0 + 4.0 * 5.0)).abs() < 1e-3, "{}", area(&half));
    }

    #[test]
    fn triangle_area_and_self_crossing() {
        let tri = vec![pos2(0.0, 0.0), pos2(10.0, 10.0), pos2(0.0, 10.0)];
        assert!((area(&fill_trapezoids(&[tri], -1.0, 11.0)) - 50.0).abs() < 1e-2);
        // A bow tie: two triangles of 25 each.
        let bow = vec![pos2(0.0, 0.0), pos2(10.0, 10.0), pos2(10.0, 0.0), pos2(0.0, 10.0)];
        let a = area(&fill_trapezoids(&[bow], -1.0, 11.0));
        assert!((a - 50.0).abs() < 1.0, "{a}");
        assert!(fill_trapezoids(&[vec![pos2(0.0, 0.0), pos2(1.0, 1.0)]], 0.0, 1.0).is_empty());
    }

    #[test]
    fn document_and_screen_round_trip() {
        let canvas = Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0));
        let view = crate::view::View { zoom: 2.0, offset: vec2(10.0, 20.0), placed: true };
        let xf = Xf::new(canvas, &view, 300.0);
        let p = Point::new(12.5, 40.0);
        let s = xf.to_screen(p);
        assert!((xf.to_doc(s) - p).hypot() < 1e-4);
        assert_eq!(xf.to_screen(Point::new(0.0, 300.0)), pos2(110.0, 70.0), "document top-left is the page's top-left");
        let a = xf.affine() * p;
        assert!((a.x as f32 - s.x).abs() < 1e-3 && (a.y as f32 - s.y).abs() < 1e-3);
        assert!((xf.mm(20.0) - 10.0).abs() < 1e-9);
        let r = xf.rect(KRect::new(0.0, 0.0, 10.0, 10.0));
        assert!((r.width() - 20.0).abs() < 1e-4 && (r.height() - 20.0).abs() < 1e-4);
    }

    #[test]
    fn black_layer_is_light_on_dark() {
        assert_eq!(ink(0, true), Color32::from_gray(220));
        assert_eq!(ink(0, false), crate::palette::layer_colour(0));
        assert_eq!(ink(1, true), crate::palette::layer_colour(1));
    }

    #[test]
    fn smooth_flattens_finer() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let view = crate::view::View { zoom: 4.0, offset: vec2(0.0, 0.0), placed: true };
        let xf = Xf::new(canvas, &view, 400.0);
        let e = darkburn_core::Shape::ellipse(KRect::new(10.0, 10.0, 60.0, 40.0), 0).outline();
        let fine = screen_polylines(&e, &xf, true);
        let coarse = screen_polylines(&e, &xf, false);
        assert!(fine[0].0.len() > coarse[0].0.len());
        assert!(fine[0].1, "an ellipse is closed");
    }
}
