//! Finding shapes: by click, by selection box, by cycling, and the Edit menu's select commands.

use kurbo::{Line, ParamCurve, PathSeg, Point, Rect, Shape as _};

use crate::document::Document;
use crate::editor::Selection;
use crate::shape::{Shape, ShapeId};

/// The topmost selectable shape at `p`: within `tolerance` of its outline, or inside it when
/// it is drawn filled (or is an image).
pub fn hit(doc: &Document, p: Point, tolerance: f64) -> Option<ShapeId> {
    doc.iter().rev().filter(|s| doc.is_selectable(s)).find(|s| hits(doc, s, p, tolerance)).map(|s| s.id)
}

/// Every selectable shape at `p`, topmost first (for click-cycling through stacked shapes).
pub fn hits_at(doc: &Document, p: Point, tolerance: f64) -> Vec<ShapeId> {
    doc.iter().rev().filter(|s| doc.is_selectable(s) && hits(doc, s, p, tolerance)).map(|s| s.id).collect()
}

fn hits(doc: &Document, s: &Shape, p: Point, tolerance: f64) -> bool {
    if !s.bbox().inflate(tolerance, tolerance).contains(p) {
        return false;
    }
    if (s.is_image() || doc.is_filled(s)) && s.contains(p) {
        return true;
    }
    // Text and codes are picked anywhere in their box.
    if matches!(s.kind, crate::shape::ShapeKind::Text(_) | crate::shape::ShapeKind::Code(_)) {
        return true;
    }
    if let crate::shape::ShapeKind::Group { children } = &s.kind {
        // Groups are hit through their children, each by its own rules.
        let local = s.transform.inverse() * p;
        let scale = s.transform.as_coeffs()[0].hypot(s.transform.as_coeffs()[1]).max(1e-9);
        return children.iter().any(|c| hits(doc, c, local, tolerance / scale));
    }
    darkburn_geom::query::distance_to_path(&s.outline(), p) <= tolerance
}

/// Box selection. Dragged left to right (`crossing` false) takes shapes wholly inside `r`;
/// right to left (`crossing` true) also takes shapes the box touches.
pub fn in_box(doc: &Document, r: Rect, crossing: bool) -> Vec<ShapeId> {
    let r = r.abs();
    doc.iter()
        .filter(|s| doc.is_selectable(s))
        .filter(|s| {
            let bb = s.bbox();
            if r.contains_rect(bb) {
                return true;
            }
            crossing && bb.overlaps(r) && touches(doc, s, r)
        })
        .map(|s| s.id)
        .collect()
}

fn touches(doc: &Document, s: &Shape, r: Rect) -> bool {
    let outline = s.outline();
    // An outline point inside the box, or an edge crossing the box's sides.
    let edges = [
        Line::new((r.x0, r.y0), (r.x1, r.y0)),
        Line::new((r.x1, r.y0), (r.x1, r.y1)),
        Line::new((r.x1, r.y1), (r.x0, r.y1)),
        Line::new((r.x0, r.y1), (r.x0, r.y0)),
    ];
    for seg in outline.segments() {
        if r.contains(seg.start()) || r.contains(seg.end()) {
            return true;
        }
        let e = PathSeg::Line;
        if edges.iter().any(|l| !darkburn_geom::query::seg_intersections(&seg, &e(*l)).is_empty()) {
            return true;
        }
    }
    // The box inside a filled shape.
    (doc.is_filled(s) || s.is_image()) && s.contains(r.center())
}

/// The next (or previous) selectable shape after the current one, in draw order (Tab).
pub fn cycle(doc: &Document, selection: &Selection, forward: bool) -> Option<ShapeId> {
    let ids: Vec<ShapeId> = doc.iter().filter(|s| doc.is_selectable(s)).map(|s| s.id).collect();
    if ids.is_empty() {
        return None;
    }
    let current = selection.last().and_then(|id| ids.iter().position(|i| *i == id));
    let n = ids.len();
    let next = match (current, forward) {
        (None, true) => 0,
        (None, false) => n - 1,
        (Some(i), true) => (i + 1) % n,
        (Some(i), false) => (i + n - 1) % n,
    };
    Some(ids[next])
}

/// Every selectable shape.
pub fn all(doc: &Document) -> Vec<ShapeId> {
    doc.iter().filter(|s| doc.is_selectable(s)).map(|s| s.id).collect()
}

/// Selectable shapes not in `selection`.
pub fn inverted(doc: &Document, selection: &Selection) -> Vec<ShapeId> {
    all(doc).into_iter().filter(|id| !selection.contains(*id)).collect()
}

/// Shapes with an open outline.
pub fn open_shapes(doc: &Document) -> Vec<ShapeId> {
    doc.iter().filter(|s| doc.is_selectable(s) && !s.is_closed()).map(|s| s.id).collect()
}

/// Open shapes on a layer set to fill (they will not fill).
pub fn open_fill_shapes(doc: &Document) -> Vec<ShapeId> {
    doc.iter().filter(|s| doc.is_selectable(s) && !s.is_closed() && s.layers().iter().any(|l| doc.layer(*l).fills())).map(|s| s.id).collect()
}

/// Shapes on `layer`.
pub fn on_layer(doc: &Document, layer: usize) -> Vec<ShapeId> {
    doc.iter().filter(|s| doc.is_selectable(s) && s.layers().contains(&layer)).map(|s| s.id).collect()
}

/// Shapes lying wholly inside one of the selected closed shapes.
pub fn contained(doc: &Document, selection: &Selection) -> Vec<ShapeId> {
    let outers: Vec<&Shape> = selection.ids().iter().filter_map(|id| doc.get(*id)).filter(|s| s.is_closed()).collect();
    doc.iter()
        .filter(|s| doc.is_selectable(s) && !selection.contains(s.id))
        .filter(|s| {
            let nodes = s.nodes();
            let bb = s.bbox();
            outers.iter().any(|o| o.bbox().contains_rect(bb) && !nodes.is_empty() && nodes.iter().all(|p| o.outline().winding(*p) != 0))
        })
        .map(|s| s.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::Mode;
    use kurbo::BezPath;

    fn doc() -> (Document, ShapeId, ShapeId, ShapeId) {
        let mut d = Document::new();
        let a = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        let b = d.add(Shape::rect(Rect::new(20.0, 0.0, 30.0, 10.0), 0.0, 1));
        let mut line = BezPath::new();
        line.move_to((40.0, 0.0));
        line.line_to((50.0, 10.0));
        let c = d.add(Shape::path(line, 1));
        (d, a, b, c)
    }

    #[test]
    fn click_on_outline_or_inside_filled() {
        let (mut d, a, b, _) = doc();
        assert_eq!(hit(&d, Point::new(0.2, 5.0), 0.5), Some(a));
        assert_eq!(hit(&d, Point::new(5.0, 5.0), 0.5), None, "line layers are hit on the outline only");
        d.layer_mut(1).main_mut().mode = Mode::Fill;
        assert_eq!(hit(&d, Point::new(25.0, 5.0), 0.5), Some(b), "fill layers are hit inside");
        assert_eq!(hit(&d, Point::new(100.0, 100.0), 0.5), None);
    }

    #[test]
    fn topmost_wins_and_stack_is_listed() {
        let mut d = Document::new();
        let low = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        let high = d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0));
        assert_eq!(hit(&d, Point::new(0.0, 5.0), 0.5), Some(high));
        assert_eq!(hits_at(&d, Point::new(0.0, 5.0), 0.5), vec![high, low]);
    }

    #[test]
    fn enclosing_and_crossing_boxes() {
        let (d, a, b, c) = doc();
        let r = Rect::new(-1.0, -1.0, 25.0, 11.0);
        assert_eq!(in_box(&d, r, false), vec![a]);
        assert_eq!(in_box(&d, r, true), vec![a, b]);
        // A box in the middle of an unfilled rectangle does not touch it.
        assert!(in_box(&d, Rect::new(2.0, 2.0, 8.0, 8.0), true).is_empty());
        assert_eq!(in_box(&d, Rect::new(44.0, 4.0, 46.0, 6.0), true), vec![c], "crossing a line");
    }

    #[test]
    fn tab_cycles_both_ways() {
        let (d, a, b, c) = doc();
        let mut s = Selection::new();
        assert_eq!(cycle(&d, &s, true), Some(a));
        assert_eq!(cycle(&d, &s, false), Some(c));
        s.set([c]);
        assert_eq!(cycle(&d, &s, true), Some(a));
        s.set([a]);
        assert_eq!(cycle(&d, &s, false), Some(c));
        s.set([b]);
        assert_eq!(cycle(&d, &s, true), Some(c));
    }

    #[test]
    fn edit_menu_selections() {
        let (mut d, a, b, c) = doc();
        assert_eq!(open_shapes(&d), vec![c]);
        assert!(open_fill_shapes(&d).is_empty());
        d.layer_mut(1).main_mut().mode = Mode::Fill;
        assert_eq!(open_fill_shapes(&d), vec![c]);
        assert_eq!(on_layer(&d, 1), vec![b, c]);
        let mut s = Selection::new();
        s.set([a]);
        assert_eq!(inverted(&d, &s), vec![b, c]);
        let inner = d.add(Shape::rect(Rect::new(2.0, 2.0, 4.0, 4.0), 0.0, 0));
        assert_eq!(contained(&d, &s), vec![inner]);
        d.get_mut(inner).unwrap().locked = true;
        assert!(contained(&d, &s).is_empty());
        assert_eq!(all(&d).len(), 3);
    }
}
