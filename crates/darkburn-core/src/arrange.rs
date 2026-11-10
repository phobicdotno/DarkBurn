//! Arranging shapes: move, scale, rotate, flip, align, distribute, draw order, grouping and
//! locking. Each function changes the document and reports whether anything changed.

use std::sync::Arc;

use kurbo::{Affine, Point, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::shape::{Shape, ShapeId, ShapeKind};

/// One of nine points of a box: corners, edge middles and the centre (the 9-point anchor).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    #[default]
    Centre,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Centre,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    /// (x, y) as 0, 0.5 or 1 across the box, y measured upwards.
    pub fn fractions(self) -> (f64, f64) {
        match self {
            Anchor::TopLeft => (0.0, 1.0),
            Anchor::Top => (0.5, 1.0),
            Anchor::TopRight => (1.0, 1.0),
            Anchor::Left => (0.0, 0.5),
            Anchor::Centre => (0.5, 0.5),
            Anchor::Right => (1.0, 0.5),
            Anchor::BottomLeft => (0.0, 0.0),
            Anchor::Bottom => (0.5, 0.0),
            Anchor::BottomRight => (1.0, 0.0),
        }
    }

    /// The anchor point of `r` (Y up: top is `y1`).
    pub fn of(self, r: Rect) -> Point {
        let (fx, fy) = self.fractions();
        Point::new(r.x0 + r.width() * fx, r.y0 + r.height() * fy)
    }
}

/// Apply `a` (in document coordinates) to the shapes `ids`.
pub fn transform(doc: &mut Document, ids: &[ShapeId], a: Affine) -> bool {
    let mut changed = false;
    for id in ids {
        if let Some(s) = doc.get_mut(*id) {
            s.transform = a * s.transform;
            changed = true;
        }
    }
    changed
}

/// Move by `d` millimetres.
pub fn translate(doc: &mut Document, ids: &[ShapeId], d: Vec2) -> bool {
    d != Vec2::ZERO && transform(doc, ids, Affine::translate(d))
}

/// `a` applied about point `p`.
pub fn about(p: Point, a: Affine) -> Affine {
    Affine::translate(p.to_vec2()) * a * Affine::translate(-p.to_vec2())
}

/// Rotate by `degrees` (counter-clockwise, Y up) about the selection centre.
pub fn rotate(doc: &mut Document, ids: &[ShapeId], degrees: f64) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    transform(doc, ids, about(bb.center(), Affine::rotate(degrees.to_radians())))
}

/// Mirror left to right about the selection centre.
pub fn flip_h(doc: &mut Document, ids: &[ShapeId]) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    transform(doc, ids, about(bb.center(), Affine::FLIP_X))
}

/// Mirror top to bottom about the selection centre.
pub fn flip_v(doc: &mut Document, ids: &[ShapeId]) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    transform(doc, ids, about(bb.center(), Affine::FLIP_Y))
}

/// Move the selection so its `anchor` point lands on `to`.
pub fn place(doc: &mut Document, ids: &[ShapeId], anchor: Anchor, to: Point) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    translate(doc, ids, to - anchor.of(bb))
}

/// Resize the selection to `width` by `height` keeping its `anchor` point still. Zero or
/// negative sizes are refused; a zero-width selection (a vertical line) keeps its width.
pub fn resize(doc: &mut Document, ids: &[ShapeId], anchor: Anchor, width: f64, height: f64) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return false;
    }
    let sx = if bb.width() > 1e-9 { width / bb.width() } else { 1.0 };
    let sy = if bb.height() > 1e-9 { height / bb.height() } else { 1.0 };
    scale(doc, ids, anchor, sx, sy)
}

/// Scale by `sx`, `sy` about the selection's `anchor` point.
pub fn scale(doc: &mut Document, ids: &[ShapeId], anchor: Anchor, sx: f64, sy: f64) -> bool {
    let Some(bb) = doc.bbox_of(ids) else { return false };
    if (sx - 1.0).abs() < 1e-12 && (sy - 1.0).abs() < 1e-12 {
        return false;
    }
    transform(doc, ids, about(anchor.of(bb), Affine::scale_non_uniform(sx, sy)))
}

/// How to align shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Top,
    Bottom,
    /// Centres on one vertical line (same X).
    HCentres,
    /// Centres on one horizontal line (same Y).
    VCentres,
    Centres,
}

/// Align the shapes `ids` to the last one (the last selected), as LightBurn-style tools do.
pub fn align(doc: &mut Document, ids: &[ShapeId], how: Align) -> bool {
    if ids.len() < 2 {
        return false;
    }
    let Some(reference) = ids.last().and_then(|id| doc.get(*id)).map(Shape::bbox) else { return false };
    let mut changed = false;
    for id in &ids[..ids.len() - 1] {
        let Some(bb) = doc.get(*id).map(Shape::bbox) else { continue };
        let d = match how {
            Align::Left => Vec2::new(reference.x0 - bb.x0, 0.0),
            Align::Right => Vec2::new(reference.x1 - bb.x1, 0.0),
            Align::Top => Vec2::new(0.0, reference.y1 - bb.y1),
            Align::Bottom => Vec2::new(0.0, reference.y0 - bb.y0),
            Align::HCentres => Vec2::new(reference.center().x - bb.center().x, 0.0),
            Align::VCentres => Vec2::new(0.0, reference.center().y - bb.center().y),
            Align::Centres => reference.center() - bb.center(),
        };
        changed |= translate(doc, &[*id], d);
    }
    changed
}

/// How to distribute shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Distribute {
    /// Equal gaps between neighbours, the outermost two stay.
    Horizontally,
    Vertically,
    /// Equally spaced centres.
    HCentres,
    VCentres,
    /// Neighbours `padding` apart, from the leftmost (bottom-most) one.
    HSpacing(f64),
    VSpacing(f64),
    /// Pushed together side by side around their common centre.
    HTogether,
    VTogether,
}

/// Distribute the shapes `ids` (needs at least two; three for the even-gap kinds).
pub fn distribute(doc: &mut Document, ids: &[ShapeId], how: Distribute) -> bool {
    let horizontal = matches!(how, Distribute::Horizontally | Distribute::HCentres | Distribute::HSpacing(_) | Distribute::HTogether);
    // (id, start, size) along the axis, sorted by start.
    let mut items: Vec<(ShapeId, f64, f64)> = ids
        .iter()
        .filter_map(|id| doc.get(*id).map(|s| (*id, s.bbox())))
        .map(|(id, bb)| if horizontal { (id, bb.x0, bb.width()) } else { (id, bb.y0, bb.height()) })
        .collect();
    if items.len() < 2 {
        return false;
    }
    items.sort_by(|a, b| a.1.total_cmp(&b.1));
    let n = items.len();
    // What the shapes span together: a wide shape can reach past the one that begins last.
    let first = items[0].1;
    let last_end = items.iter().map(|i| i.1 + i.2).fold(f64::NEG_INFINITY, f64::max);
    let total: f64 = items.iter().map(|i| i.2).sum();
    // The even kinds go by the centres: the shapes with the outermost centres are the ends.
    if matches!(how, Distribute::Horizontally | Distribute::Vertically | Distribute::HCentres | Distribute::VCentres) {
        items.sort_by(|a, b| (a.1 + a.2 / 2.0).total_cmp(&(b.1 + b.2 / 2.0)));
    }
    let starts: Vec<f64> = match how {
        Distribute::Horizontally | Distribute::Vertically => {
            if n < 3 {
                return false;
            }
            let gap = (last_end - first - total) / (n - 1) as f64;
            running(&items, first, gap)
        }
        Distribute::HCentres | Distribute::VCentres => {
            if n < 3 {
                return false;
            }
            let c0 = items[0].1 + items[0].2 / 2.0;
            let c1 = items[n - 1].1 + items[n - 1].2 / 2.0;
            items.iter().enumerate().map(|(i, it)| c0 + (c1 - c0) * i as f64 / (n - 1) as f64 - it.2 / 2.0).collect()
        }
        Distribute::HSpacing(p) | Distribute::VSpacing(p) => running(&items, first, p),
        Distribute::HTogether | Distribute::VTogether => {
            let centre = (first + last_end) / 2.0;
            running(&items, centre - total / 2.0, 0.0)
        }
    };
    let mut changed = false;
    for (item, start) in items.iter().zip(starts) {
        let d = start - item.1;
        changed |= translate(doc, &[item.0], if horizontal { Vec2::new(d, 0.0) } else { Vec2::new(0.0, d) });
    }
    changed
}

fn running(items: &[(ShapeId, f64, f64)], from: f64, gap: f64) -> Vec<f64> {
    let mut at = from;
    items
        .iter()
        .map(|it| {
            let s = at;
            at += it.2 + gap;
            s
        })
        .collect()
}

/// Make every shape as wide (or tall) as the last selected, about each one's centre.
pub fn same_size(doc: &mut Document, ids: &[ShapeId], width: bool) -> bool {
    let Some(reference) = ids.last().and_then(|id| doc.get(*id)).map(Shape::bbox) else { return false };
    // A reference with no width (a vertical line) or no height has no size to copy: scaling to
    // it would flatten the other shapes for good.
    let target = if width { reference.width() } else { reference.height() };
    if !(target.is_finite() && target > 1e-9) {
        return false;
    }
    let mut changed = false;
    for id in &ids[..ids.len().saturating_sub(1)] {
        let Some(bb) = doc.get(*id).map(Shape::bbox) else { continue };
        let (sx, sy) = if width {
            (if bb.width() > 1e-9 { reference.width() / bb.width() } else { 1.0 }, 1.0)
        } else {
            (1.0, if bb.height() > 1e-9 { reference.height() / bb.height() } else { 1.0 })
        };
        changed |= scale(doc, &[*id], Anchor::Centre, sx, sy);
    }
    changed
}

/// Move the selection to one of 9 positions on the page `page` (a rect from 0,0).
pub fn to_page(doc: &mut Document, ids: &[ShapeId], page: Rect, anchor: Anchor) -> bool {
    place(doc, ids, anchor, anchor.of(page))
}

/// Draw order changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Forward,
    Backward,
    Front,
    Back,
}

/// Change the draw order of `ids`.
pub fn reorder(doc: &mut Document, ids: &[ShapeId], how: Order) -> bool {
    let before: Vec<ShapeId> = doc.shapes.iter().map(|s| s.id).collect();
    let picked = |s: &Arc<Shape>| ids.contains(&s.id);
    match how {
        Order::Front | Order::Back => {
            let (mut sel, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut doc.shapes).into_iter().partition(picked);
            doc.shapes = if how == Order::Front {
                let mut v = rest;
                v.append(&mut sel);
                v
            } else {
                sel.extend(rest);
                sel
            };
        }
        Order::Forward => {
            for i in (0..doc.shapes.len().saturating_sub(1)).rev() {
                if picked(&doc.shapes[i]) && !picked(&doc.shapes[i + 1]) {
                    doc.shapes.swap(i, i + 1);
                }
            }
        }
        Order::Backward => {
            for i in 1..doc.shapes.len() {
                if picked(&doc.shapes[i]) && !picked(&doc.shapes[i - 1]) {
                    doc.shapes.swap(i, i - 1);
                }
            }
        }
    }
    doc.shapes.iter().map(|s| s.id).collect::<Vec<_>>() != before
}

/// Group the shapes `ids` into one group at the place of the topmost; returns its id.
pub fn group(doc: &mut Document, ids: &[ShapeId]) -> Option<ShapeId> {
    let mut picked: Vec<(usize, Shape)> =
        doc.shapes.iter().enumerate().filter(|(_, s)| ids.contains(&s.id)).map(|(i, s)| (i, Shape::clone(s))).collect();
    if picked.len() < 2 {
        return None;
    }
    let top = picked.last().map(|p| p.0).unwrap_or(0);
    let layer = picked[0].1.layer;
    let removed = picked.len();
    doc.remove(ids);
    let children: Vec<Shape> = picked.drain(..).map(|(_, s)| s).collect();
    let mut g = Shape::new(ShapeKind::Group { children }, Affine::IDENTITY, layer);
    g.id = doc.alloc_id();
    let id = g.id;
    doc.shapes.insert((top + 1 - removed).min(doc.shapes.len()), Arc::new(g));
    Some(id)
}

/// Ungroup the groups among `ids`; returns the ids of the freed shapes (and of `ids` that
/// were not groups).
pub fn ungroup(doc: &mut Document, ids: &[ShapeId]) -> Vec<ShapeId> {
    let mut out = Vec::new();
    for id in ids {
        let Some(i) = doc.index_of(*id) else { continue };
        let s = Shape::clone(&doc.shapes[i]);
        if let ShapeKind::Group { children } = s.kind {
            doc.shapes.remove(i);
            for (k, mut c) in children.into_iter().enumerate() {
                c.transform = s.transform * c.transform;
                out.push(c.id);
                doc.shapes.insert(i + k, Arc::new(c));
            }
        } else {
            out.push(*id);
        }
    }
    out
}

/// Lock or unlock shapes.
pub fn set_locked(doc: &mut Document, ids: &[ShapeId], locked: bool) -> bool {
    let mut changed = false;
    for id in ids {
        if let Some(s) = doc.get(*id)
            && s.locked != locked
            && let Some(s) = doc.get_mut(*id)
        {
            s.locked = locked;
            changed = true;
        }
    }
    changed
}

/// Put shapes on `layer`.
pub fn set_layer(doc: &mut Document, ids: &[ShapeId], layer: usize) -> bool {
    let mut changed = false;
    for id in ids {
        if doc.get(*id).is_some_and(|s| s.layers() != vec![layer])
            && let Some(s) = doc.get_mut(*id)
        {
            s.set_layer(layer);
            changed = true;
        }
    }
    changed
}

/// Delete shapes.
pub fn delete(doc: &mut Document, ids: &[ShapeId]) -> bool {
    let n = doc.remove(ids);
    doc.prune_images();
    n > 0
}

/// Copies of `ids` placed on top, shifted by `offset`; returns the new ids.
pub fn duplicate(doc: &mut Document, ids: &[ShapeId], offset: Vec2) -> Vec<ShapeId> {
    let copies: Vec<Shape> = doc.iter().filter(|s| ids.contains(&s.id)).cloned().collect();
    copies
        .into_iter()
        .map(|mut s| {
            s.transform = Affine::translate(offset) * s.transform;
            doc.add(s)
        })
        .collect()
}

/// Rotation in degrees of a shape's transform (counter-clockwise).
pub fn rotation_of(a: Affine) -> f64 {
    let c = a.as_coeffs();
    c[1].atan2(c[0]).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn three() -> (Document, Vec<ShapeId>) {
        let mut d = Document::new();
        let ids = vec![
            d.add(Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0)),
            d.add(Shape::rect(Rect::new(30.0, 5.0, 40.0, 25.0), 0.0, 0)),
            d.add(Shape::rect(Rect::new(100.0, 50.0, 120.0, 60.0), 0.0, 0)),
        ];
        (d, ids)
    }

    fn bb(d: &Document, id: ShapeId) -> Rect {
        d.get(id).unwrap().bbox()
    }

    #[test]
    fn anchors_pick_box_points() {
        let r = Rect::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(Anchor::TopLeft.of(r), Point::new(0.0, 20.0));
        assert_eq!(Anchor::BottomRight.of(r), Point::new(10.0, 0.0));
        assert_eq!(Anchor::Centre.of(r), Point::new(5.0, 10.0));
    }

    #[test]
    fn move_resize_scale_rotate_flip() {
        let (mut d, ids) = three();
        let a = &ids[..1];
        assert!(place(&mut d, a, Anchor::BottomLeft, Point::new(50.0, 50.0)));
        assert!(near(bb(&d, ids[0]).x0, 50.0) && near(bb(&d, ids[0]).y0, 50.0));
        assert!(resize(&mut d, a, Anchor::BottomLeft, 20.0, 5.0));
        let b = bb(&d, ids[0]);
        assert!(near(b.width(), 20.0) && near(b.height(), 5.0) && near(b.x0, 50.0) && near(b.y0, 50.0));
        assert!(!resize(&mut d, a, Anchor::Centre, 0.0, 5.0));
        assert!(rotate(&mut d, a, 90.0));
        let r = bb(&d, ids[0]);
        assert!(near(r.width(), 5.0) && near(r.height(), 20.0));
        assert!(near(rotation_of(d.get(ids[0]).unwrap().transform), 90.0));
        let before = bb(&d, ids[0]);
        assert!(flip_h(&mut d, a));
        assert!(flip_v(&mut d, a));
        let after = bb(&d, ids[0]);
        assert!(near(before.x0, after.x0) && near(before.y1, after.y1), "flips keep the box");
    }

    #[test]
    fn align_to_last_selected() {
        let (mut d, ids) = three();
        assert!(align(&mut d, &ids, Align::Left));
        assert!(ids.iter().all(|id| near(bb(&d, *id).x0, 100.0)));
        assert!(align(&mut d, &ids, Align::Top));
        assert!(ids.iter().all(|id| near(bb(&d, *id).y1, 60.0)));
        assert!(align(&mut d, &ids, Align::Centres));
        assert!(ids.iter().all(|id| (bb(&d, *id).center() - Point::new(110.0, 55.0)).hypot() < 1e-6));
        assert!(!align(&mut d, &ids[..1], Align::Right));
    }

    #[test]
    fn distribute_gaps_centres_spacing_together() {
        let (mut d, ids) = three();
        assert!(distribute(&mut d, &ids, Distribute::Horizontally));
        let g1 = bb(&d, ids[1]).x0 - bb(&d, ids[0]).x1;
        let g2 = bb(&d, ids[2]).x0 - bb(&d, ids[1]).x1;
        assert!(near(g1, g2));
        assert!(distribute(&mut d, &ids, Distribute::HSpacing(2.0)));
        assert!(near(bb(&d, ids[1]).x0 - bb(&d, ids[0]).x1, 2.0));
        assert!(distribute(&mut d, &ids, Distribute::VCentres));
        let c: Vec<f64> = ids.iter().map(|id| bb(&d, *id).center().y).collect();
        let mut sorted = c.clone();
        sorted.sort_by(f64::total_cmp);
        assert!(near(sorted[1] - sorted[0], sorted[2] - sorted[1]));
        assert!(distribute(&mut d, &ids, Distribute::VTogether));
        let mut spans: Vec<Rect> = ids.iter().map(|id| bb(&d, *id)).collect();
        spans.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        assert!(near(spans[0].y1, spans[1].y0) && near(spans[1].y1, spans[2].y0));
        assert!(!distribute(&mut d, &ids[..2], Distribute::Horizontally), "even gaps need three");
    }

    fn row(spans: &[(f64, f64)]) -> (Document, Vec<ShapeId>) {
        let mut d = Document::new();
        let ids = spans.iter().map(|(x0, x1)| d.add(Shape::rect(Rect::new(*x0, 0.0, *x1, 10.0), 0.0, 0))).collect();
        (d, ids)
    }

    fn span(d: &Document, id: ShapeId) -> (f64, f64) {
        let b = bb(d, id);
        ((b.x0 * 1e6).round() / 1e6, (b.x1 * 1e6).round() / 1e6)
    }

    /// Centres 50, 15 and 35: the outer two centres (15 and 50) stay and the one between them goes
    /// to the middle, 32.5. The wide shape begins furthest left, but its centre is furthest right.
    #[test]
    fn distributing_centres_goes_by_the_centres_not_the_left_edges() {
        let (mut d, ids) = row(&[(0.0, 100.0), (10.0, 20.0), (30.0, 40.0)]);
        assert!(distribute(&mut d, &ids, Distribute::HCentres));
        assert_eq!(span(&d, ids[0]), (0.0, 100.0));
        assert_eq!(span(&d, ids[1]), (10.0, 20.0));
        assert_eq!(span(&d, ids[2]), (27.5, 37.5));
    }

    /// Shapes 0..10, 20..100 and 30..40 span 0..100 and are 100 wide together: with even gaps
    /// (none here) they still span 0..100.
    #[test]
    fn distributing_keeps_the_span_of_the_selection_when_a_wide_shape_reaches_past_the_last() {
        let (mut d, ids) = row(&[(0.0, 10.0), (20.0, 100.0), (30.0, 40.0)]);
        assert!(distribute(&mut d, &ids, Distribute::Horizontally));
        assert_eq!(span(&d, ids[0]), (0.0, 10.0));
        assert_eq!(span(&d, ids[2]), (10.0, 20.0));
        assert_eq!(span(&d, ids[1]), (20.0, 100.0));
        // Moved together: side by side about the middle of what they spanned (50), 110 wide.
        let (mut d, ids) = row(&[(0.0, 100.0), (10.0, 20.0)]);
        assert!(distribute(&mut d, &ids, Distribute::HTogether));
        assert_eq!(span(&d, ids[0]), (-5.0, 95.0));
        assert_eq!(span(&d, ids[1]), (95.0, 105.0));
    }

    #[test]
    fn page_positions_and_same_size() {
        let (mut d, ids) = three();
        let page = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert!(to_page(&mut d, &ids[..1], page, Anchor::TopRight));
        assert!(near(bb(&d, ids[0]).x1, 400.0) && near(bb(&d, ids[0]).y1, 300.0));
        assert!(same_size(&mut d, &ids, true));
        assert!(ids.iter().all(|id| near(bb(&d, *id).width(), 20.0)));
    }

    #[test]
    fn same_size_against_a_line_with_no_width_leaves_the_shapes_alone() {
        let (mut d, mut ids) = three();
        let mut upright = kurbo::BezPath::new();
        upright.move_to((5.0, 0.0));
        upright.line_to((5.0, 30.0));
        ids.push(d.add(Shape::path(upright, 0)));
        let before: Vec<Rect> = ids.iter().map(|id| bb(&d, *id)).collect();
        assert!(!same_size(&mut d, &ids, true), "there is no width to copy");
        let after: Vec<Rect> = ids.iter().map(|id| bb(&d, *id)).collect();
        assert_eq!(before, after);
        // Its height is a real size and is still copied.
        assert!(same_size(&mut d, &ids, false));
        assert!(ids.iter().all(|id| near(bb(&d, *id).height(), 30.0)));
    }

    #[test]
    fn draw_order() {
        let (mut d, ids) = three();
        let order = |d: &Document| d.shapes.iter().map(|s| s.id).collect::<Vec<_>>();
        assert!(reorder(&mut d, &ids[..1], Order::Front));
        assert_eq!(order(&d), vec![ids[1], ids[2], ids[0]]);
        assert!(reorder(&mut d, &ids[..1], Order::Backward));
        assert_eq!(order(&d), vec![ids[1], ids[0], ids[2]]);
        assert!(reorder(&mut d, &ids[..1], Order::Back));
        assert_eq!(order(&d), vec![ids[0], ids[1], ids[2]]);
        assert!(reorder(&mut d, &ids[..1], Order::Forward));
        assert_eq!(order(&d), vec![ids[1], ids[0], ids[2]]);
        assert!(!reorder(&mut d, &ids[2..], Order::Front));
    }

    #[test]
    fn group_ungroup_keeps_geometry() {
        let (mut d, ids) = three();
        let before: Vec<Rect> = ids.iter().map(|id| bb(&d, *id)).collect();
        let g = group(&mut d, &ids[..2]).unwrap();
        assert_eq!(d.len(), 2);
        assert!(rotate(&mut d, &[g], 180.0));
        assert!(rotate(&mut d, &[g], 180.0));
        let freed = ungroup(&mut d, &[g]);
        assert_eq!(freed, vec![ids[0], ids[1]]);
        for (id, b) in ids.iter().zip(&before) {
            let a = bb(&d, *id);
            assert!(near(a.x0, b.x0) && near(a.y1, b.y1));
        }
        assert!(group(&mut d, &ids[..1]).is_none());
    }

    #[test]
    fn lock_layer_delete_duplicate() {
        let (mut d, ids) = three();
        assert!(set_locked(&mut d, &ids[..1], true));
        assert!(!set_locked(&mut d, &ids[..1], true));
        assert!(set_layer(&mut d, &ids, 7));
        assert_eq!(d.used_layers(), vec![7]);
        let copies = duplicate(&mut d, &ids[1..], Vec2::new(1.0, 0.0));
        assert_eq!(copies.len(), 2);
        assert!(near(bb(&d, copies[0]).x0, 31.0));
        assert!(delete(&mut d, &copies));
        assert_eq!(d.len(), 3);
    }
}
