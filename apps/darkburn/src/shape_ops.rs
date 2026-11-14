//! Shape-editing commands: convert to path, close, join, optimise, booleans, offset, cut,
//! slots, warp and deform, rounded corners, mirror and the rest of the Edit, Tools and Arrange
//! menus. Every one is a registered command; arguments are JSON (a missing argument takes a
//! default), each runs without a window, and each edit is one undo step.
//!
//! Destructive operations replace shapes with `ShapeKind::Path` outlines on the same layer and
//! under the same name. All geometry comes from `darkburn_geom`. Arrays, nesting and docking are
//! in `shape_arrays.rs`; node editing, tabs, start points, measuring and lines in
//! `shape_nodes.rs`.

use darkburn_core::arrange;
use darkburn_core::expr;
use darkburn_core::kurbo::{Affine, BezPath, Point, Rect};
use darkburn_core::{Document, Editor, Selection, Shape, ShapeId, ShapeKind};
use darkburn_geom::FillRule;
use darkburn_geom::boolean;
use darkburn_geom::cleanup;
use darkburn_geom::fillet;
use darkburn_geom::hull;
use darkburn_geom::offset::{self, Cap, Join, OffsetOptions, OffsetSide};
use darkburn_geom::path as gpath;
use darkburn_geom::query;
use darkburn_geom::slots::{self, Slot, SlotKind, SlotQuery};
use darkburn_geom::transform as gxf;
use serde_json::{Value, json};

use crate::shell::DarkBurn;
use crate::tools::Tool;

/// Default flattening and join tolerance in millimetres.
pub const TOL: f64 = 0.01;

/// What the commands need to know about the window.
#[derive(Clone, Copy, Debug)]
pub struct Env {
    pub page: Rect,
    pub padding: f64,
    pub dock_as_group: bool,
    pub dock_lock_inner: bool,
    pub corner_radius: f64,
}

pub type R = Result<Value, String>;

/// What a command that would make shapes further out than a design can reach answers.
pub const OUT_OF_REACH: &str = "That lies further out than a design can reach: nothing was made.";

// ---------------------------------------------------------------- argument helpers

pub fn num(a: &Value, key: &str, default: f64) -> f64 {
    match a.get(key) {
        Some(Value::Number(n)) => n.as_f64().filter(|v| v.is_finite()).unwrap_or(default),
        Some(Value::String(s)) => expr::eval_in(s, 1.0).filter(|v| v.is_finite()).unwrap_or(default),
        _ => default,
    }
}

pub fn flag(a: &Value, key: &str, default: bool) -> bool {
    a.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub fn word<'a>(a: &'a Value, key: &str, default: &'a str) -> &'a str {
    a.get(key).and_then(Value::as_str).unwrap_or(default)
}

pub fn point(a: &Value, key: &str) -> Option<Point> {
    let v = a.get(key)?;
    if let Some(arr) = v.as_array() {
        return Some(Point::new(arr.first()?.as_f64()?, arr.get(1)?.as_f64()?));
    }
    Some(Point::new(v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}

pub fn ids_arg(a: &Value, key: &str) -> Option<Vec<ShapeId>> {
    Some(a.get(key)?.as_array()?.iter().filter_map(Value::as_u64).map(ShapeId).collect())
}

pub fn ok(changed: bool, ids: &[ShapeId]) -> R {
    Ok(json!({ "changed": changed, "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>() }))
}

// ---------------------------------------------------------------- shape helpers

/// A path taken from a shape: document coordinates plus what the result keeps.
#[derive(Clone, Debug)]
pub struct Part {
    pub path: BezPath,
    pub layer: usize,
    pub name: String,
}

/// The outline parts of shape `id` (a group gives one part per descendant; images none).
pub fn parts_of(doc: &Document, id: ShapeId) -> Vec<Part> {
    let Some(s) = doc.get(id) else { return Vec::new() };
    s.leaves().into_iter().filter(|l| !l.is_image()).map(|l| Part { path: l.outline(), layer: l.layer, name: l.name.clone() }).collect()
}

/// `ids` that exist and can be edited, bottom of the draw order first.
pub fn in_draw_order(doc: &Document, ids: &[ShapeId]) -> Vec<ShapeId> {
    doc.iter().filter(|s| ids.contains(&s.id) && !s.locked).map(|s| s.id).collect()
}

pub fn path_shape(path: BezPath, layer: usize, name: &str) -> Shape {
    let mut s = Shape::path(path, layer);
    s.name = name.to_string();
    s
}

/// Take `consumed` out of the document and put `new` where the lowest of them was; the new
/// shapes become the selection. A picture inside a group that is used up has no outline that
/// took part: it stays, out of its group, under the new shapes.
pub fn replace(doc: &mut Document, sel: &mut Selection, consumed: &[ShapeId], new: Vec<Shape>) -> Vec<ShapeId> {
    let mut at = consumed.iter().filter_map(|i| doc.index_of(*i)).min().unwrap_or(doc.len());
    let pictures: Vec<Shape> =
        consumed.iter().filter_map(|i| doc.get(*i)).filter(|s| s.is_group()).flat_map(Shape::leaves).filter(Shape::is_image).collect();
    doc.remove(consumed);
    for p in pictures {
        doc.insert(at, p);
        at += 1;
    }
    let out: Vec<ShapeId> = new.into_iter().enumerate().map(|(k, s)| doc.insert(at + k, s)).collect();
    sel.set(out.iter().copied());
    out
}

fn selected(e: &Editor) -> Result<Vec<ShapeId>, String> {
    let ids = in_draw_order(&e.doc, &e.selected());
    if ids.is_empty() { Err("nothing is selected".into()) } else { Ok(ids) }
}

/// Run `f` over the shape (or every descendant of a group) turning each result into a path in
/// the shape's parent space with an identity transform. Returns how many shapes changed.
pub fn map_shape(s: &mut Shape, f: &mut dyn FnMut(&Shape) -> Option<BezPath>) -> usize {
    if let ShapeKind::Group { children } = &mut s.kind {
        let n = children.iter_mut().map(|c| map_shape(c, f)).sum();
        if n > 0 {
            s.array = None;
        }
        return n;
    }
    if s.is_image() {
        return 0;
    }
    match f(s) {
        Some(p) => {
            let old = s.transform;
            // Tabs stay on the outline: each moves to the nearest point of the new one.
            let onto = |t: &[f64; 2]| {
                let q = old * Point::new(t[0], t[1]);
                let q = query::nearest_point(&p, q).map_or(q, |h| h.point);
                [q.x, q.y]
            };
            let mut tabs: Vec<[f64; 2]> = Vec::new();
            for t in s.tabs.iter().map(onto) {
                if !tabs.iter().any(|u| Point::new(u[0], u[1]).distance(Point::new(t[0], t[1])) < 1e-3) {
                    tabs.push(t);
                }
            }
            s.tabs = tabs;
            let starts: Vec<[f64; 2]> = s.start_points().map(|t| old * Point::new(t[0], t[1])).map(|p| [p.x, p.y]).collect();
            s.set_start_points(&starts);
            s.kind = ShapeKind::Path { path: p };
            s.transform = Affine::IDENTITY;
            1
        }
        None => 0,
    }
}

pub fn map_shapes(doc: &mut Document, ids: &[ShapeId], f: &mut dyn FnMut(&Shape) -> Option<BezPath>) -> usize {
    let mut n = 0;
    for id in ids {
        if let Some(s) = doc.get_mut(*id) {
            n += map_shape(s, f);
        }
    }
    n
}

/// [`map_shapes`] for work that is measured in the document (a warp to given corners, a radius
/// or a tolerance in millimetres): `f` gets each shape and its outline where it lies in the
/// document and answers there, also for a shape in a group that was moved, turned or scaled
/// after it was made.
pub fn map_shapes_in_doc(doc: &mut Document, ids: &[ShapeId], f: &mut dyn FnMut(&Shape, &BezPath) -> Option<BezPath>) -> usize {
    fn walk(s: &mut Shape, parent: Affine, f: &mut dyn FnMut(&Shape, &BezPath) -> Option<BezPath>) -> usize {
        if let ShapeKind::Group { children } = &mut s.kind {
            let own = parent * s.transform;
            let n = children.iter_mut().map(|c| walk(c, own, f)).sum();
            if n > 0 {
                s.array = None;
            }
            return n;
        }
        if parent == Affine::IDENTITY {
            return map_shape(s, &mut |s| f(s, &s.outline()));
        }
        // A group squashed flat has no way back from the document: its shapes are left alone.
        if !parent.is_finite() || parent.determinant().abs() < 1e-12 {
            return 0;
        }
        let back = parent.inverse();
        map_shape(s, &mut |s| {
            let mut p = s.outline();
            p.apply_affine(parent);
            let mut q = f(s, &p)?;
            q.apply_affine(back);
            Some(q)
        })
    }
    let mut n = 0;
    for id in ids {
        if let Some(s) = doc.get_mut(*id) {
            n += walk(s, Affine::IDENTITY, f);
        }
    }
    n
}

/// The outline of a plain shape in document coordinates (None for groups and images).
pub fn shape_path(doc: &Document, id: ShapeId) -> Option<BezPath> {
    let s = doc.get(id)?;
    (!s.is_group() && !s.is_image()).then(|| s.outline())
}

/// Give shape `id` the document-space outline `path` (it becomes a path shape). A `path` with
/// nothing left to draw (its last nodes were deleted) takes the shape out of the document.
pub fn set_shape_path(doc: &mut Document, id: ShapeId, path: BezPath) -> bool {
    if gpath::contours(&path).is_empty() {
        return doc.remove(&[id]) > 0;
    }
    let Some(s) = doc.get_mut(id) else { return false };
    map_shape(s, &mut |_| Some(path.clone())) > 0
}

fn tol_arg(a: &Value) -> f64 {
    // Nothing or nonsense (zero, below zero, not a number) is the usual tolerance, and the finest
    // one is a tenth of a micrometre: finer makes outlines of millions of points for nothing.
    let t = num(a, "tolerance", TOL);
    if t > 0.0 { t.clamp(1e-4, 100.0) } else { TOL }
}

fn bbox_of_ids(doc: &Document, ids: &[ShapeId]) -> Option<Rect> {
    doc.bbox_of(ids)
}

// ---------------------------------------------------------------- convert, close, join

fn convert_path(e: &mut Editor, _a: &Value) -> R {
    let ids = selected(e)?;
    let mut n = 0;
    let done = e.edit("Convert to path", |d, _| {
        crate::text_render::refresh(d);
        // A shape with nothing to draw (a text of spaces) stays what it is: as a path it would
        // be an empty shape nobody can see, select by clicking or type into.
        n = map_shapes(d, &ids, &mut |s| (!matches!(s.kind, ShapeKind::Path { .. })).then(|| s.outline()).filter(|o| !gpath::contours(o).is_empty()));
        n > 0
    });
    Ok(json!({ "changed": done, "converted": n }))
}

fn close_path(e: &mut Editor, a: &Value, tolerance: bool) -> R {
    let ids = selected(e)?;
    let tol = num(a, "tolerance", 0.1).clamp(1e-6, 1000.0);
    let mut n = 0;
    let done = e.edit("Close path", |d, _| {
        n = map_shapes_in_doc(d, &ids, &mut |s, p| {
            if !matches!(s.kind, ShapeKind::Path { .. }) {
                return None;
            }
            let q = if tolerance { cleanup::close_paths_with_tolerance(p, tol) } else { cleanup::close_path(p) };
            (q != *p).then_some(q)
        });
        n > 0
    });
    Ok(json!({ "changed": done, "closed": n }))
}

/// Open paths of the selection grouped by layer: (layer, shape ids, their paths).
pub fn join_inputs(doc: &Document, ids: &[ShapeId]) -> Vec<(usize, Vec<ShapeId>, Vec<BezPath>)> {
    let mut by_layer: Vec<(usize, Vec<ShapeId>, Vec<BezPath>)> = Vec::new();
    for id in ids {
        let Some(s) = doc.get(*id) else { continue };
        if !matches!(s.kind, ShapeKind::Path { .. }) {
            continue;
        }
        let p = s.outline();
        match by_layer.iter_mut().find(|g| g.0 == s.layer) {
            Some(g) => {
                g.1.push(*id);
                g.2.push(p);
            }
            None => by_layer.push((s.layer, vec![*id], vec![p])),
        }
    }
    by_layer
}

/// Replace the shapes of `groups` (from [`join_inputs`]) by the joined paths `results`, one list
/// per group, in one undo step.
pub fn apply_joined(e: &mut Editor, groups: &[(usize, Vec<ShapeId>, Vec<BezPath>)], results: Vec<Vec<BezPath>>) -> R {
    let mut all = Vec::new();
    let before: usize = groups.iter().map(|g| g.1.len()).sum();
    let done = e.edit("Auto-join", |d, sel| {
        for (g, res) in groups.iter().zip(results) {
            let name = d.get(g.1[0]).map(|s| s.name.clone()).unwrap_or_default();
            let new: Vec<Shape> = res.into_iter().map(|p| path_shape(p, g.0, &name)).collect();
            all.extend(replace(d, sel, &g.1, new));
        }
        sel.set(all.iter().copied());
        true
    });
    Ok(json!({ "changed": done, "before": before, "after": all.len(), "ids": all.iter().map(|i| i.0).collect::<Vec<_>>() }))
}

fn auto_join(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let tol = num(a, "tolerance", 0.1).clamp(1e-6, 1000.0);
    let groups = join_inputs(&e.doc, &ids);
    if groups.is_empty() {
        return Err("select open paths to join".into());
    }
    let results: Vec<Vec<BezPath>> = groups.iter().map(|g| cleanup::auto_join(&g.2, tol, &mut |_, _| true).unwrap_or_else(|| g.2.clone())).collect();
    apply_joined(e, &groups, results)
}

fn optimize(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let tol = num(a, "tolerance", 0.01).clamp(1e-6, 100.0);
    let (mut before, mut after) = (0, 0);
    let done = e.edit("Optimize", |d, _| {
        let n = map_shapes_in_doc(d, &ids, &mut |s, p| {
            if !matches!(s.kind, ShapeKind::Path { .. }) {
                return None;
            }
            let (q, r) = cleanup::optimize(p, tol);
            before += r.nodes_before;
            after += r.nodes_after;
            (q != *p).then_some(q)
        });
        n > 0
    });
    Ok(json!({ "changed": done, "nodes_before": before, "nodes_after": after }))
}

fn delete_duplicates(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let tol = num(a, "tolerance", 0.01).clamp(1e-6, 100.0);
    let mut gone = Vec::new();
    for (_, gids, paths) in join_inputs_all(&e.doc, &ids) {
        let (kept, removed) = cleanup::delete_duplicates(&paths, tol);
        if removed == 0 {
            continue;
        }
        let mut k = 0;
        for (id, p) in gids.iter().zip(&paths) {
            if kept.get(k) == Some(p) {
                k += 1;
            } else {
                gone.push(*id);
            }
        }
    }
    let done = e.edit("Delete duplicates", |d, _| arrange::delete(d, &gone));
    Ok(json!({ "changed": done, "removed": gone.len() }))
}

/// Like [`join_inputs`] but for every plain shape kind (rectangles and the like too).
fn join_inputs_all(doc: &Document, ids: &[ShapeId]) -> Vec<(usize, Vec<ShapeId>, Vec<BezPath>)> {
    let mut by_layer: Vec<(usize, Vec<ShapeId>, Vec<BezPath>)> = Vec::new();
    for id in ids {
        let Some(s) = doc.get(*id) else { continue };
        if s.is_group() || s.is_image() {
            continue;
        }
        let p = s.outline();
        match by_layer.iter_mut().find(|g| g.0 == s.layer) {
            Some(g) => {
                g.1.push(*id);
                g.2.push(p);
            }
            None => by_layer.push((s.layer, vec![*id], vec![p])),
        }
    }
    by_layer
}

fn delete_overlapping(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let dist = num(a, "distance", 0.01).clamp(1e-6, 100.0);
    let groups = join_inputs(&e.doc, &ids);
    if groups.is_empty() {
        return Err("select line paths".into());
    }
    let results: Vec<Vec<BezPath>> = groups.iter().map(|g| cleanup::delete_overlapping_lines(&g.2, dist)).collect();
    let unchanged = groups.iter().zip(&results).all(|(g, r)| &g.2 == r);
    if unchanged {
        return ok(false, &[]);
    }
    apply_joined(e, &groups, results)
}

fn break_apart(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let keep_holes = flag(a, "keep_holes", false);
    let mut out = Vec::new();
    let done = e.edit("Break apart", |d, sel| {
        for id in &ids {
            let parts = parts_of(d, *id);
            let mut new = Vec::new();
            for p in &parts {
                for piece in cleanup::break_apart(&p.path, keep_holes, TOL) {
                    new.push(path_shape(piece, p.layer, &p.name));
                }
            }
            if new.len() > 1 || (new.len() == 1 && d.get(*id).is_some_and(Shape::is_group)) {
                out.extend(replace(d, sel, &[*id], new));
            }
        }
        sel.set(out.iter().copied());
        !out.is_empty()
    });
    ok(done, &out)
}

fn auto_group(e: &mut Editor, _a: &Value) -> R {
    let mut ids = in_draw_order(&e.doc, &e.selected());
    if ids.is_empty() {
        ids = in_draw_order(&e.doc, &darkburn_core::select::all(&e.doc));
    }
    let cand: Vec<(ShapeId, BezPath)> = ids
        .iter()
        .filter_map(|id| {
            let s = e.doc.get(*id)?;
            (!s.is_image() && !s.is_group() && s.is_closed()).then(|| (*id, s.outline()))
        })
        .collect();
    let paths: Vec<BezPath> = cand.iter().map(|c| c.1.clone()).collect();
    let tree = query::containment(&paths, TOL);
    fn gather(tree: &query::ContainmentTree, i: usize, out: &mut Vec<usize>) {
        out.push(i);
        for c in &tree.children[i] {
            gather(tree, *c, out);
        }
    }
    let mut made = Vec::new();
    let done = e.edit("Auto-group", |d, sel| {
        for root in tree.roots() {
            let mut members = Vec::new();
            gather(&tree, root, &mut members);
            if members.len() < 2 {
                continue;
            }
            let gids: Vec<ShapeId> = members.iter().map(|m| cand[*m].0).collect();
            if let Some(g) = arrange::group(d, &gids) {
                made.push(g);
            }
        }
        sel.set(made.iter().copied());
        !made.is_empty()
    });
    ok(done, &made)
}

// ---------------------------------------------------------------- offset

/// The settings of Offset Shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetParams {
    pub distance: f64,
    pub side: OffsetSide,
    pub join: Join,
    pub delete_original: bool,
    pub select_results: bool,
    pub outer_only: bool,
    pub open_shapes: bool,
    pub tolerance: f64,
}

impl Default for OffsetParams {
    fn default() -> Self {
        OffsetParams {
            distance: 1.0,
            side: OffsetSide::Outward,
            join: Join::Round,
            delete_original: false,
            select_results: true,
            outer_only: false,
            open_shapes: false,
            tolerance: TOL,
        }
    }
}

impl OffsetParams {
    pub fn from_json(a: &Value) -> Result<OffsetParams, String> {
        let d = OffsetParams::default();
        let side = match word(a, "side", "outward") {
            "outward" | "outside" => OffsetSide::Outward,
            "inward" | "inside" => OffsetSide::Inward,
            "both" => OffsetSide::Both,
            other => return Err(format!("unknown side {other}")),
        };
        let join = match word(a, "join", "round") {
            "round" => Join::Round,
            "bevel" => Join::Bevel,
            "sharp" | "miter" => Join::Miter { limit: 4.0 },
            other => return Err(format!("unknown join {other}")),
        };
        let mut p = OffsetParams {
            distance: num(a, "distance", d.distance),
            side,
            join,
            delete_original: flag(a, "delete_original", d.delete_original),
            select_results: flag(a, "select_results", d.select_results),
            outer_only: flag(a, "outer_only", d.outer_only),
            open_shapes: flag(a, "open_shapes", d.open_shapes),
            tolerance: tol_arg(a),
        };
        if p.distance < 0.0 {
            p.distance = -p.distance;
            p.side = match p.side {
                OffsetSide::Outward => OffsetSide::Inward,
                OffsetSide::Inward => OffsetSide::Outward,
                OffsetSide::Both => OffsetSide::Both,
            };
        }
        if p.distance < 1e-9 {
            return Err("the offset distance must not be zero".into());
        }
        Ok(p)
    }
}

/// Offset results of shapes `ids`: one `(path, layer, name)` per result.
pub fn offset_results(doc: &Document, ids: &[ShapeId], p: &OffsetParams) -> Vec<Part> {
    offset_results_by_shape(doc, ids, p).into_iter().flat_map(|(_, parts)| parts).collect()
}

/// The offset results of each shape of `ids` (none for an image, or for an open line when open
/// shapes are not offset).
fn offset_results_by_shape(doc: &Document, ids: &[ShapeId], p: &OffsetParams) -> Vec<(ShapeId, Vec<Part>)> {
    let opts = OffsetOptions {
        distance: p.distance,
        side: p.side,
        join: p.join,
        cap: Cap::Round,
        outer_only: p.outer_only,
        fill_rule: FillRule::EvenOdd,
        tol: p.tolerance,
    };
    let mut by_shape = Vec::new();
    for id in ids {
        let mut out = Vec::new();
        for part in parts_of(doc, *id) {
            let subs = gpath::subpaths(&part.path);
            let (closed, open): (Vec<_>, Vec<_>) = subs.into_iter().partition(|s| gpath::is_closed(s, 1e-6));
            let mut results = Vec::new();
            if !closed.is_empty() {
                results.extend(offset::offset(&closed, &opts).all());
            }
            if p.open_shapes {
                for o in &open {
                    // Open lines grow to both sides; "inward only" has no meaning for them.
                    results.extend(offset::offset_open(o, p.distance, p.join, Cap::Round, p.tolerance));
                }
            }
            out.extend(results.into_iter().map(|path| Part { path, layer: part.layer, name: part.name.clone() }));
        }
        by_shape.push((*id, out));
    }
    by_shape
}

/// What Offset would draw, for the dialog's live preview.
pub fn offset_preview(doc: &Document, ids: &[ShapeId], p: &OffsetParams) -> Vec<BezPath> {
    offset_results(doc, ids, p).into_iter().map(|r| r.path).collect()
}

fn offset_cmd(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let p = OffsetParams::from_json(a)?;
    let by_shape = offset_results_by_shape(&e.doc, &ids, &p);
    // Only a shape that was offset is replaced by its offset; an image or an open line that
    // gave nothing stays.
    let ids: Vec<ShapeId> = by_shape.iter().filter(|(_, parts)| !parts.is_empty()).map(|(id, _)| *id).collect();
    let results: Vec<Part> = by_shape.into_iter().flat_map(|(_, parts)| parts).collect();
    if results.is_empty() {
        return Err("the offset left nothing (the distance may be too large)".into());
    }
    let mut out = Vec::new();
    e.edit("Offset", |d, sel| {
        let new: Vec<Shape> = results.iter().map(|r| path_shape(r.path.clone(), r.layer, &r.name)).collect();
        if p.delete_original {
            out = replace(d, sel, &ids, new);
        } else {
            out = new.into_iter().map(|s| d.add(s)).collect();
        }
        if p.select_results {
            sel.set(out.iter().copied());
        }
        true
    });
    ok(true, &out)
}

// ---------------------------------------------------------------- booleans

fn operands(doc: &Document, ids: &[ShapeId]) -> Vec<(ShapeId, Vec<Part>)> {
    ids.iter().map(|id| (*id, parts_of(doc, *id))).filter(|(_, p)| !p.is_empty()).collect()
}

/// The shapes of `ids` that enclose an area, with their outlines: what a boolean operation
/// works on. An image, a straight line or anything else without an area takes no part in it
/// and is left alone.
fn area_operands(doc: &Document, ids: &[ShapeId]) -> Vec<(ShapeId, Vec<Part>)> {
    operands(doc, ids).into_iter().filter(|(_, parts)| parts.iter().any(|p| gpath::area(&p.path, FillRule::EvenOdd, TOL) > 1e-9)).collect()
}

fn ids_of(ops: &[(ShapeId, Vec<Part>)]) -> Vec<ShapeId> {
    ops.iter().map(|o| o.0).collect()
}

fn paths_of(parts: &[Part]) -> Vec<BezPath> {
    parts.iter().map(|p| p.path.clone()).collect()
}

/// The paths a boolean operation gives for `a` and `b` (each a set of shapes).
pub fn boolean_paths(a: &[BezPath], b: &[BezPath], op: &str) -> Result<Vec<BezPath>, String> {
    let rule = FillRule::EvenOdd;
    Ok(match op {
        "union" => {
            let all: Vec<BezPath> = a.iter().chain(b).cloned().collect();
            boolean::union(&all, rule, TOL)
        }
        "difference" => boolean::difference(a, b, rule, TOL),
        "intersection" => boolean::intersection(a, b, rule, TOL),
        "xor" => boolean::xor(a, b, rule, TOL),
        other => return Err(format!("unknown boolean operation {other}")),
    })
}

/// Preview of the boolean assistant: operation `op` of shape sets `a` and `b`.
pub fn boolean_preview(doc: &Document, a: &[ShapeId], b: &[ShapeId], op: &str) -> Vec<BezPath> {
    let pa: Vec<BezPath> = a.iter().flat_map(|i| parts_of(doc, *i)).map(|p| p.path).collect();
    let pb: Vec<BezPath> = b.iter().flat_map(|i| parts_of(doc, *i)).map(|p| p.path).collect();
    boolean_paths(&pa, &pb, op).unwrap_or_default()
}

fn boolean_cmd(e: &mut Editor, a: &Value, op: &str) -> R {
    let ops = area_operands(&e.doc, &selected(e)?);
    let ids = ids_of(&ops);
    if ops.len() < 2 {
        return Err("select at least two closed shapes".into());
    }
    // The bottom shape is the subject; the shapes above it cut into it (difference) or are
    // combined with it.
    let first = &ops[0].1;
    let result: Vec<BezPath> = match op {
        "union" => boolean_paths(&paths_of(first), &ops[1..].iter().flat_map(|o| paths_of(&o.1)).collect::<Vec<_>>(), "union")?,
        "difference" => boolean_paths(&paths_of(first), &ops[1..].iter().flat_map(|o| paths_of(&o.1)).collect::<Vec<_>>(), "difference")?,
        "weld" => {
            let all: Vec<BezPath> = ops.iter().flat_map(|o| paths_of(&o.1)).collect();
            let w = boolean::weld(&all, FillRule::EvenOdd, TOL);
            if w.elements().is_empty() { Vec::new() } else { vec![w] }
        }
        "intersection" => {
            let mut acc = paths_of(first);
            for o in &ops[1..] {
                acc = boolean_paths(&acc, &paths_of(&o.1), "intersection")?;
            }
            acc
        }
        other => return Err(format!("unknown boolean operation {other}")),
    };
    let _ = a;
    finish_boolean(e, &ids, &ops[0].1[0], result, "Boolean")
}

fn finish_boolean(e: &mut Editor, consumed: &[ShapeId], first: &Part, result: Vec<BezPath>, label: &str) -> R {
    if result.is_empty() {
        return Err("the result is empty, nothing was changed".into());
    }
    let mut out = Vec::new();
    e.edit(label, |d, sel| {
        let new: Vec<Shape> = result.into_iter().map(|p| path_shape(p, first.layer, &first.name)).collect();
        out = replace(d, sel, consumed, new);
        true
    });
    ok(true, &out)
}

fn boolean_assistant(e: &mut Editor, a: &Value) -> R {
    let ids_a = ids_arg(a, "a").ok_or("argument a must be a list of shape ids")?;
    let ids_b = ids_arg(a, "b").ok_or("argument b must be a list of shape ids")?;
    let op = word(a, "op", "difference");
    let keep = flag(a, "keep_originals", false);
    let pa: Vec<Part> = ids_a.iter().flat_map(|i| parts_of(&e.doc, *i)).collect();
    let pb: Vec<Part> = ids_b.iter().flat_map(|i| parts_of(&e.doc, *i)).collect();
    if pa.is_empty() || pb.is_empty() {
        return Err("both sets need at least one shape".into());
    }
    let result = boolean_paths(&paths_of(&pa), &paths_of(&pb), op)?;
    if keep {
        if result.is_empty() {
            return Err("the result is empty".into());
        }
        let mut out = Vec::new();
        e.edit("Boolean assistant", |d, sel| {
            out = result.into_iter().map(|p| d.add(path_shape(p, pa[0].layer, &pa[0].name))).collect();
            sel.set(out.iter().copied());
            true
        });
        return ok(true, &out);
    }
    let chosen: Vec<ShapeId> = ids_a.iter().chain(&ids_b).copied().collect();
    let consumed = ids_of(&area_operands(&e.doc, &chosen));
    finish_boolean(e, &consumed, &pa[0], result, "Boolean assistant")
}

fn cut_shapes(e: &mut Editor, _a: &Value) -> R {
    let ops = area_operands(&e.doc, &selected(e)?);
    let ids = ids_of(&ops);
    if ops.len() < 2 {
        return Err("select at least two closed shapes".into());
    }
    let all: Vec<BezPath> = ops.iter().flat_map(|o| paths_of(&o.1)).collect();
    let pieces = boolean::cut_shapes(&all, FillRule::EvenOdd, TOL);
    finish_boolean(e, &ids, &ops[0].1[0], pieces, "Cut shapes")
}

// ---------------------------------------------------------------- slots

/// A slot found in a shape, with a quadrilateral for highlighting it.
#[derive(Clone, Debug)]
pub struct SlotHit {
    pub shape: ShapeId,
    pub slot: Slot,
    /// Outline of the slot's mouth-to-bottom area, document mm.
    pub quad: BezPath,
}

pub fn slot_query(a: &Value) -> SlotQuery {
    let d = SlotQuery::default();
    SlotQuery {
        min_width: num(a, "min_width", d.min_width),
        max_width: num(a, "max_width", f64::INFINITY),
        min_depth: num(a, "min_depth", d.min_depth),
        max_depth: num(a, "max_depth", f64::INFINITY),
        tol: num(a, "slot_tolerance", d.tol),
        angle_tol_deg: d.angle_tol_deg,
        include_tabs: flag(a, "include_tabs", false),
    }
}

fn slot_quad(s: &Slot) -> BezPath {
    let n = darkburn_core::kurbo::Vec2::new(-s.axis.y, s.axis.x);
    let (a, b) = (s.mouth - n * (s.width / 2.0), s.mouth + n * (s.width / 2.0));
    let depth = s.axis * s.depth;
    let mut p = BezPath::new();
    p.move_to(a);
    p.line_to(b);
    p.line_to(b + depth);
    p.line_to(a + depth);
    p.close_path();
    p
}

/// Every slot of the shapes `ids` that matches `q`.
pub fn find_slots_in(doc: &Document, ids: &[ShapeId], q: &SlotQuery) -> Vec<SlotHit> {
    let mut out = Vec::new();
    for id in ids {
        for part in parts_of(doc, *id) {
            for slot in slots::find_slots(&part.path, q) {
                out.push(SlotHit { shape: *id, quad: slot_quad(&slot), slot });
            }
        }
    }
    out
}

fn resize_slots_cmd(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let q = slot_query(a);
    let nw = a.get("new_width").map(|_| num(a, "new_width", 0.0));
    let nd = a.get("new_depth").map(|_| num(a, "new_depth", 0.0));
    if nw.is_none() && nd.is_none() {
        let hits = find_slots_in(&e.doc, &ids, &q);
        let list: Vec<Value> = hits
            .iter()
            .map(|h| json!({ "shape": h.shape.0, "width": h.slot.width, "depth": h.slot.depth, "round": h.slot.kind == SlotKind::Round, "tab": h.slot.protruding }))
            .collect();
        return Ok(json!({ "changed": false, "slots": list }));
    }
    let mut count = 0;
    let done = e.edit("Resize slots", |d, _| {
        let n = map_shapes_in_doc(d, &ids, &mut |_, p| {
            let found = slots::find_slots(p, &q);
            if found.is_empty() {
                return None;
            }
            count += found.len();
            Some(slots::resize_slots(p, &found, nw, nd))
        });
        n > 0
    });
    Ok(json!({ "changed": done, "resized": count }))
}

fn select_smaller(e: &mut Editor, _a: &Value) -> R {
    let ids = e.selected();
    let reference = ids.iter().filter_map(|i| e.doc.get(*i)).map(Shape::bbox).reduce(|a, b| a.union(b)).ok_or("select a shape first")?;
    let found: Vec<ShapeId> = e
        .doc
        .iter()
        .filter(|s| !s.locked && !ids.contains(&s.id) && e.doc.is_selectable(s))
        .filter(|s| {
            let b = s.bbox();
            b.width() <= reference.width() + 1e-9
                && b.height() <= reference.height() + 1e-9
                && (b.width() * b.height()) < reference.width() * reference.height() - 1e-9
        })
        .map(|s| s.id)
        .collect();
    e.selection.set(found.iter().copied());
    ok(false, &found)
}

// ---------------------------------------------------------------- warp, deform, corners

/// A point a design can reach: finite and no further out than the editor allows.
fn within_reach(p: Point) -> bool {
    let max = darkburn_core::editor::MAX_COORD;
    p.x.is_finite() && p.y.is_finite() && p.x.abs() <= max && p.y.abs() <= max
}

fn warp(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let src = bbox_of_ids(&e.doc, &ids).ok_or("nothing to warp")?;
    let corners = match a.get("corners").and_then(Value::as_array) {
        Some(c) if c.len() == 4 => {
            let mut pts = [Point::ZERO; 4];
            for (i, v) in c.iter().enumerate() {
                let arr = v.as_array().filter(|x| x.len() == 2).ok_or("corners must be four [x, y] points")?;
                pts[i] = reachable(Point::new(arr[0].as_f64().ok_or("bad corner")?, arr[1].as_f64().ok_or("bad corner")?))?;
            }
            pts
        }
        _ => return Err("corners must list four [x, y] points: bottom-left, bottom-right, top-right, top-left".into()),
    };
    if !corners.iter().all(|p| within_reach(*p)) {
        return Err(OUT_OF_REACH.into());
    }
    let perspective = word(a, "mode", "perspective") == "perspective";
    let mut failed = false;
    let done = e.edit("Warp", |d, _| {
        map_shapes_in_doc(d, &ids, &mut |_, p| {
            let r = if perspective { gxf::warp_perspective(p, src, corners, TOL) } else { gxf::warp_bilinear(p, src, corners, TOL) };
            failed |= r.is_none();
            r
        }) > 0
    });
    if failed && !done {
        return Err("the corners do not make a convex shape".into());
    }
    ok(done, &ids)
}

/// A warp or deform point the shapes may be stretched to: one further out than the document allows
/// would be refused anyway, after a long time spent bending the shapes towards it.
fn reachable(p: Point) -> Result<Point, String> {
    let max = darkburn_core::editor::MAX_COORD;
    if p.x.abs() <= max && p.y.abs() <= max { Ok(p) } else { Err(OUT_OF_REACH.into()) }
}

fn deform(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let src = bbox_of_ids(&e.doc, &ids).ok_or("nothing to deform")?;
    let pts = a.get("grid").and_then(Value::as_array).ok_or("grid must list 16 [x, y] points, bottom row first")?;
    if pts.len() != 16 {
        return Err("grid must list 16 points".into());
    }
    let mut grid = [[Point::ZERO; 4]; 4];
    for (i, v) in pts.iter().enumerate() {
        let arr = v.as_array().filter(|x| x.len() == 2).ok_or("grid points are [x, y]")?;
        grid[i / 4][i % 4] = reachable(Point::new(arr[0].as_f64().ok_or("bad point")?, arr[1].as_f64().ok_or("bad point")?))?;
    }
    // A point beyond reach could only give a shape nobody can reach, and flattening the mapping
    // of such a point asks for gigabytes.
    if !grid.iter().flatten().all(|p| within_reach(*p)) {
        return Err(OUT_OF_REACH.into());
    }
    let done = e.edit("Deform", |d, _| map_shapes_in_doc(d, &ids, &mut |_, p| gxf::deform_grid(p, src, grid, TOL)) > 0);
    ok(done, &ids)
}

fn round_corners(e: &mut Editor, a: &Value, env: &Env) -> R {
    let ids = selected(e)?;
    let radius = num(a, "radius", env.corner_radius);
    if radius.abs() < 1e-9 {
        return Err("the radius must not be zero".into());
    }
    let chamfer = flag(a, "chamfer", false);
    let nodes: Option<Vec<darkburn_geom::node::NodeRef>> = a.get("nodes").and_then(Value::as_array).map(|l| {
        l.iter()
            .filter_map(|n| {
                let n = n.as_array()?;
                Some(darkburn_geom::node::NodeRef::new(n.first()?.as_u64()? as usize, n.get(1)?.as_u64()? as usize))
            })
            .collect()
    });
    let done = e.edit("Round corners", |d, _| {
        map_shapes_in_doc(d, &ids, &mut |_, p| {
            let q = match (&nodes, chamfer) {
                (None, false) => fillet::round_corners(p, radius, TOL),
                (Some(n), false) => fillet::round_corners_at(p, n, radius, TOL),
                (None, true) => fillet::chamfer(p, radius.abs(), TOL),
                (Some(n), true) => fillet::chamfer_at(p, n, radius.abs(), TOL),
            };
            (q != *p).then_some(q)
        }) > 0
    });
    ok(done, &ids)
}

// ---------------------------------------------------------------- mirror, two-point, rubber band

fn straight_line(doc: &Document, id: ShapeId) -> Option<(Point, Point)> {
    let s = doc.get(id)?;
    let p = shape_path(doc, id)?;
    let _ = s;
    let cs = gpath::contours(&p);
    let c = cs.first()?;
    (cs.len() == 1 && !c.closed && c.segs.len() == 1 && matches!(c.segs[0], darkburn_core::kurbo::PathSeg::Line(_)))
        .then(|| (c.start().unwrap_or_default(), c.end().unwrap_or_default()))
}

fn mirror(e: &mut Editor, a: &Value) -> R {
    let mut ids = selected(e)?;
    let line = match (point(a, "a"), point(a, "b")) {
        (Some(p), Some(q)) => (p, q),
        _ => {
            let axis = ids.iter().rev().find_map(|id| straight_line(&e.doc, *id).map(|l| (*id, l))).filter(|_| ids.len() >= 2);
            let (axis_id, l) = axis.ok_or("select the shapes and a straight line to mirror across, or pass points a and b")?;
            ids.retain(|i| *i != axis_id);
            l
        }
    };
    let m = gxf::mirror_across_line(line.0, line.1);
    let copy = flag(a, "copy", false);
    let mut out = Vec::new();
    let done = e.edit("Mirror across line", |d, sel| {
        if copy {
            out = arrange::duplicate(d, &ids, darkburn_core::kurbo::Vec2::ZERO);
            sel.set(out.iter().copied());
            arrange::transform(d, &out, m)
        } else {
            out = ids.clone();
            arrange::transform(d, &ids, m)
        }
    });
    ok(done, &out)
}

fn two_point(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let (fa, fb, ta, tb) = (point(a, "from_a"), point(a, "from_b"), point(a, "to_a"), point(a, "to_b"));
    let (Some(fa), Some(fb), Some(ta), Some(tb)) = (fa, fb, ta, tb) else {
        return Err("from_a, from_b, to_a and to_b are all needed ([x, y])".into());
    };
    let m = gxf::two_point_rotate_scale(fa, fb, ta, tb, flag(a, "uniform", true))
        .ok_or("the two source points are the same, or the two target points are")?;
    let done = e.edit("Two-point rotate and scale", |d, _| arrange::transform(d, &ids, m));
    ok(done, &ids)
}

fn rubber_band(e: &mut Editor, a: &Value) -> R {
    let ids = selected(e)?;
    let ops = operands(&e.doc, &ids);
    let paths: Vec<BezPath> = ops.iter().flat_map(|o| paths_of(&o.1)).collect();
    let band = hull::rubber_band_outline(&paths, tol_arg(a));
    if band.elements().is_empty() {
        return Err("nothing to outline".into());
    }
    let first = &ops[0].1[0];
    let mut out = Vec::new();
    e.edit("Rubber-band outline", |d, sel| {
        out = vec![d.add(path_shape(band.clone(), first.layer, ""))];
        sel.set(out.iter().copied());
        true
    });
    ok(true, &out)
}

// ---------------------------------------------------------------- registry glue

/// JSON schema (as text) of the arguments of the commands in this module.
pub fn args_schema(id: &str) -> Option<&'static str> {
    Some(match id {
        "tools.offset" => {
            r#"{"type":"object","properties":{"distance":{"description":"mm, or text with units and equations such as 2in/4"},"side":{"enum":["outward","inward","both"]},"join":{"enum":["round","bevel","sharp"]},"delete_original":{"type":"boolean"},"select_results":{"type":"boolean"},"outer_only":{"type":"boolean"},"open_shapes":{"type":"boolean","description":"also offset open lines"},"tolerance":{"type":"number"}}}"#
        }
        "tools.boolean_assistant" => {
            r#"{"type":"object","properties":{"op":{"enum":["union","difference","intersection","xor"]},"a":{"type":"array","items":{"type":"integer"}},"b":{"type":"array","items":{"type":"integer"}},"keep_originals":{"type":"boolean"}},"required":["a","b"]}"#
        }
        "tools.resize_slots" => {
            r#"{"type":"object","properties":{"min_width":{"type":"number"},"max_width":{"type":"number"},"min_depth":{"type":"number"},"max_depth":{"type":"number"},"include_tabs":{"type":"boolean"},"new_width":{"type":"number"},"new_depth":{"type":"number"}},"description":"without new_width or new_depth the slots found are listed"}"#
        }
        "tools.warp" => {
            r#"{"type":"object","properties":{"corners":{"type":"array","description":"four [x, y] points: bottom-left, bottom-right, top-right, top-left"},"mode":{"enum":["perspective","bilinear"]}},"required":["corners"]}"#
        }
        "tools.deform" => {
            r#"{"type":"object","properties":{"grid":{"type":"array","description":"16 [x, y] points, bottom row first"}},"required":["grid"]}"#
        }
        "tools.round_corners" => {
            r#"{"type":"object","properties":{"radius":{"type":"number","description":"negative cuts inwards"},"chamfer":{"type":"boolean"},"nodes":{"type":"array","description":"[subpath, node] pairs; all corners when missing"}}}"#
        }
        "edit.close_tolerance" => r#"{"type":"object","properties":{"tolerance":{"type":"number","description":"mm"}}}"#,
        "edit.auto_join" => r#"{"type":"object","properties":{"tolerance":{"type":"number","description":"mm"}}}"#,
        "edit.optimize" | "edit.delete_duplicates" => r#"{"type":"object","properties":{"tolerance":{"type":"number","description":"mm"}}}"#,
        "edit.delete_overlapping" => r#"{"type":"object","properties":{"distance":{"type":"number","description":"mm"}}}"#,
        "arrange.break_apart" => r#"{"type":"object","properties":{"keep_holes":{"type":"boolean"}}}"#,
        "arrange.mirror" => {
            r#"{"type":"object","properties":{"a":{"type":"array","description":"[x, y] on the mirror line"},"b":{"type":"array","description":"[x, y] on the mirror line"},"copy":{"type":"boolean"}},"description":"without a and b the selected straight line is the axis"}"#
        }
        "arrange.two_point" => {
            r#"{"type":"object","properties":{"from_a":{"type":"array"},"from_b":{"type":"array"},"to_a":{"type":"array"},"to_b":{"type":"array"},"uniform":{"type":"boolean"}},"required":["from_a","from_b","to_a","to_b"]}"#
        }
        "arrange.rubber_band" => r#"{"type":"object","properties":{"tolerance":{"type":"number"}}}"#,
        _ => return crate::shape_arrays::args_schema(id).or_else(|| crate::shape_nodes::args_schema(id)),
    })
}

/// The commands that run in this build.
pub fn works(id: &str) -> bool {
    matches!(
        id,
        "edit.convert_path"
            | "edit.close_path"
            | "edit.close_tolerance"
            | "edit.auto_join"
            | "edit.optimize"
            | "edit.delete_duplicates"
            | "edit.delete_overlapping"
            | "tools.offset"
            | "tools.weld"
            | "tools.union"
            | "tools.difference"
            | "tools.intersection"
            | "tools.boolean_assistant"
            | "tools.cut_shapes"
            | "tools.resize_slots"
            | "tools.select_smaller"
            | "tools.warp"
            | "tools.deform"
            | "tools.round_corners"
            | "arrange.break_apart"
            | "arrange.move_selection"
            | "arrange.auto_group"
            | "arrange.mirror"
            | "arrange.two_point"
            | "arrange.rubber_band"
    ) || crate::shape_arrays::works(id)
        || crate::shape_nodes::works(id)
}

/// The checklist items a command in this module implements.
pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "edit.convert_path" => &["EDIT-11", "SHAPE-01"],
        "edit.close_path" => &["EDIT-13", "SHAPE-03"],
        "edit.close_tolerance" => &["EDIT-14", "SHAPE-04"],
        "edit.auto_join" => &["EDIT-15", "SHAPE-05"],
        "edit.optimize" => &["EDIT-16", "SHAPE-06"],
        "edit.delete_duplicates" => &["EDIT-17", "SHAPE-07"],
        "edit.delete_overlapping" => &["SHAPE-08"],
        "tools.offset" => &["TOOLS-12", "SHAPE-13", "SHAPE-14", "SHAPE-15", "SHAPE-16"],
        "tools.weld" => &["TOOLS-13", "SHAPE-10"],
        "tools.union" => &["TOOLS-14", "SHAPE-11"],
        "tools.difference" => &["TOOLS-15", "SHAPE-11"],
        "tools.intersection" => &["TOOLS-16", "SHAPE-11"],
        "tools.boolean_assistant" => &["TOOLS-17", "SHAPE-12"],
        "tools.cut_shapes" => &["TOOLS-18", "SHAPE-17"],
        "tools.resize_slots" => &["TOOLS-23", "SHAPE-20", "SHAPE-21"],
        "tools.select_smaller" => &["TOOLS-24"],
        "tools.warp" => &["TOOLS-25", "SHAPE-22"],
        "tools.deform" => &["TOOLS-26", "SHAPE-23"],
        "tools.round_corners" => &["TB-11", "SHAPE-19"],
        "arrange.break_apart" => &["ARR-34", "SHAPE-09"],
        "arrange.move_selection" => &["TB-06"],
        "arrange.auto_group" => &["ARR-03", "ARRAY-11"],
        "arrange.mirror" => &["ARR-06", "SHAPE-25"],
        "arrange.two_point" => &["ARR-09", "SHAPE-24"],
        "arrange.rubber_band" => &["ARR-33", "SHAPE-27"],
        "arrange.same_width" | "arrange.same_height" => &["TB-06", "SHAPE-26"],
        _ => {
            let c = crate::shape_arrays::checklist(id);
            if c.is_empty() { crate::shape_nodes::checklist(id) } else { c }
        }
    }
}

/// Whether the command has something to act on now (None: not one of ours).
pub fn can_run(app: &DarkBurn, id: &str) -> Option<bool> {
    if !works(id) {
        return None;
    }
    let sel = app.editor.selection.len();
    Some(match id {
        "tools.union" | "tools.difference" | "tools.intersection" | "tools.cut_shapes" | "tools.weld" | "arrange.nest" => {
            sel >= 2 || (id == "arrange.nest" && sel >= 1)
        }
        "tools.boolean_assistant" => true,
        "arrange.mirror" | "arrange.alignment_helper" | "arrange.move_selection" => sel >= 1,
        "arrange.array_edit"
        | "arrange.array_add"
        | "arrange.array_remove"
        | "arrange.array_exit"
        | "arrange.array_flatten"
        | "arrange.array_duplicate" => crate::shape_arrays::can_run(app, id),
        "arrange.auto_group" => true,
        "tools.nodes" | "tools.tabs" | "tools.measure" | "tools.start_points" | "tools.round_corners" => true,
        // These take their points from the arguments, not from the selection.
        "measure.points" | "measure.arc" | "shape.add_line" => true,
        _ => sel >= 1,
    })
}

fn env_of(app: &DarkBurn) -> Env {
    Env {
        page: Rect::new(0.0, 0.0, app.prefs.page_width as f64, app.prefs.page_height as f64),
        padding: app.padding,
        dock_as_group: app.shape_ui.dock_as_group,
        dock_lock_inner: app.shape_ui.dock_lock_inner,
        corner_radius: app.shape_ui.corner_radius,
    }
}

/// Run `id` on `e` with JSON `a` (None when this module does not have the command).
pub fn exec(e: &mut Editor, env: &Env, id: &str, a: &Value) -> Option<R> {
    let null = Value::Object(Default::default());
    let a = if a.is_null() { &null } else { a };
    Some(match id {
        "edit.convert_path" => convert_path(e, a),
        "edit.close_path" => close_path(e, a, false),
        "edit.close_tolerance" => close_path(e, a, true),
        "edit.auto_join" => auto_join(e, a),
        "edit.optimize" => optimize(e, a),
        "edit.delete_duplicates" => delete_duplicates(e, a),
        "edit.delete_overlapping" => delete_overlapping(e, a),
        "tools.offset" => offset_cmd(e, a),
        "tools.weld" => boolean_cmd(e, a, "weld"),
        "tools.union" => boolean_cmd(e, a, "union"),
        "tools.difference" => boolean_cmd(e, a, "difference"),
        "tools.intersection" => boolean_cmd(e, a, "intersection"),
        "tools.boolean_assistant" => boolean_assistant(e, a),
        "tools.cut_shapes" => cut_shapes(e, a),
        "tools.resize_slots" => resize_slots_cmd(e, a),
        "tools.select_smaller" => select_smaller(e, a),
        "tools.warp" => warp(e, a),
        "tools.deform" => deform(e, a),
        "tools.round_corners" => round_corners(e, a, env),
        "arrange.break_apart" => break_apart(e, a),
        "arrange.auto_group" => auto_group(e, a),
        "arrange.mirror" => mirror(e, a),
        "arrange.two_point" => two_point(e, a),
        "arrange.rubber_band" => rubber_band(e, a),
        _ => return crate::shape_arrays::exec(e, env, id, a).or_else(|| crate::shape_nodes::exec(e, env, id, a)),
    })
}

/// The headless entry (`api.rs`): run `id` with `args`.
pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Option<R> {
    if is_tool_pick(id) {
        return None;
    }
    let env = env_of(app);
    exec(&mut app.editor, &env, id, args)
}

/// Commands that only pick a canvas tool or open a dialog from the interface.
fn is_tool_pick(id: &str) -> bool {
    matches!(id, "tools.nodes" | "tools.tabs" | "tools.measure" | "tools.start_points" | "tools.alignment_helper" | "arrange.move_selection")
}

/// Run `id` from a menu, button or key: dialog commands open their dialog, tool commands pick
/// the tool, the rest run with their defaults. Returns whether `id` was one of ours.
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    if !works(id) {
        return false;
    }
    if let Some(&dialog) = crate::shape_ui::DIALOG_COMMANDS.iter().find(|d| **d == id) {
        app.shape_ui.open_dialog(dialog);
        return true;
    }
    let tool = match id {
        "tools.nodes" => Some(Tool::Nodes),
        "tools.tabs" => Some(Tool::Tabs),
        "tools.measure" => Some(Tool::Measure),
        "tools.start_points" => Some(Tool::StartPoints),
        "tools.round_corners" => Some(Tool::RoundCorners),
        "tools.warp" => Some(Tool::Warp),
        "tools.deform" => Some(Tool::Deform),
        "arrange.two_point" => Some(Tool::TwoPoint),
        "arrange.mirror" => {
            let ids = app.editor.selected();
            let has_axis = ids.len() >= 2 && ids.iter().any(|i| straight_line(&app.editor.doc, *i).is_some());
            (!has_axis).then_some(Tool::Mirror)
        }
        _ => None,
    };
    if let Some(t) = tool {
        app.set_tool(t);
        return true;
    }
    let env = env_of(app);
    let args = crate::shape_arrays::run_args(app, id);
    match exec(&mut app.editor, &env, id, &args) {
        Some(Ok(v)) => {
            app.shape_ui.status = status_text(id, &v);
        }
        Some(Err(m)) => app.shape_ui.status = Some(m),
        None => return false,
    }
    true
}

fn status_text(id: &str, v: &Value) -> Option<String> {
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    match id {
        "edit.optimize" => Some(format!("Nodes {} to {}", n("nodes_before")?, n("nodes_after")?)),
        "edit.delete_duplicates" => Some(format!("Removed {} duplicates", n("removed")?)),
        "edit.convert_path" => Some(format!("Converted {} shapes to paths", n("converted")?)),
        "arrange.nest" => Some(format!("Placed {} shapes, {} did not fit", n("placed")?, n("unplaced")?)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;

    pub fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    pub fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    pub fn add_rect(app: &mut DarkBurn, x: f64, y: f64, w: f64, h: f64, layer: usize) -> ShapeId {
        let r = run(app, "shape.add_rect", json!({ "x": x, "y": y, "width": w, "height": h, "layer": layer })).unwrap();
        ShapeId(r["id"].as_u64().unwrap())
    }

    pub fn select(app: &mut DarkBurn, ids: &[ShapeId]) {
        run(app, "selection.set", json!({ "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>() })).unwrap();
    }

    fn area(app: &DarkBurn) -> f64 {
        app.editor.doc.iter().map(|s| gpath::area(&s.outline(), FillRule::EvenOdd, TOL)).sum()
    }

    #[test]
    fn booleans_and_offset_leave_the_shapes_that_took_no_part() {
        use crate::images::testutil::{add_image, png};
        let scene = |app: &mut DarkBurn| {
            let a = add_rect(app, 0.0, 0.0, 10.0, 10.0, 0);
            let b = add_rect(app, 5.0, 5.0, 10.0, 10.0, 0);
            let image = add_image(&mut app.editor.doc, png(4, 4, |_, _| 0), (4, 4), (10.0, 10.0), (5.0, 5.0), 3);
            let line =
                ShapeId(run(app, "shape.add_line", json!({ "points": [[100, 100], [120, 100]], "connect": false })).unwrap()["id"].as_u64().unwrap());
            (a, b, image, line)
        };
        for cmd in ["tools.union", "tools.difference", "tools.weld", "tools.cut_shapes"] {
            let mut app = app();
            let (a, b, image, line) = scene(&mut app);
            select(&mut app, &[a, b, image, line]);
            run(&mut app, cmd, json!({})).unwrap();
            let d = &app.editor.doc;
            assert!(d.get(image).is_some(), "{cmd} keeps the picture that was selected with the shapes");
            assert!(d.get(line).is_some(), "{cmd} keeps the line");
            assert!(d.get(a).is_none() && d.get(b).is_none(), "{cmd} replaces the two squares");
            app.editor.undo();
            assert_eq!(app.editor.doc.len(), 4);
        }
        let mut one = app();
        let (a, b, image, line) = scene(&mut one);
        run(&mut one, "tools.boolean_assistant", json!({ "a": [a.0, image.0], "b": [b.0, line.0], "op": "union" })).unwrap();
        assert!(one.editor.doc.get(image).is_some() && one.editor.doc.get(line).is_some());
        assert_eq!(one.editor.doc.len(), 3);
        // A picture and a line alone are nothing to combine.
        select(&mut one, &[image, line]);
        assert!(run(&mut one, "tools.union", json!({})).is_err());
        // Offset with "delete original": only what was offset is replaced.
        let mut two = app();
        let (a, b, image, line) = scene(&mut two);
        select(&mut two, &[a, image, line]);
        run(&mut two, "tools.offset", json!({ "distance": 1, "delete_original": true })).unwrap();
        let d = &two.editor.doc;
        assert!(d.get(a).is_none(), "the square was replaced by its offset");
        assert!(d.get(image).is_some() && d.get(line).is_some() && d.get(b).is_some());
        assert_eq!(d.len(), 4);
    }

    #[test]
    fn offset_grows_and_one_undo_restores() {
        let mut app = app();
        let r = add_rect(&mut app, 10.0, 10.0, 20.0, 20.0, 3);
        select(&mut app, &[r]);
        let out = run(&mut app, "tools.offset", json!({ "distance": 2, "join": "sharp", "delete_original": true })).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1);
        let s = app.editor.doc.iter().next().unwrap();
        assert_eq!(s.layer, 3, "the result keeps the layer");
        assert!(matches!(s.kind, ShapeKind::Path { .. }));
        let bb = s.bbox();
        assert!((bb.width() - 24.0).abs() < 0.05, "{bb:?}");
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert!(matches!(app.editor.doc.iter().next().unwrap().kind, ShapeKind::Rect { .. }), "a single undo step");
        select(&mut app, &[r]);
        run(&mut app, "tools.offset", json!({ "distance": "1in/25.4 * 3", "side": "inward", "delete_original": false })).unwrap();
        assert_eq!(app.editor.doc.len(), 2);
        let inner = app.editor.doc.iter().last().unwrap().bbox();
        assert!((inner.width() - 14.0).abs() < 0.05, "units and equations in the distance: {inner:?}");
        assert!(run(&mut app, "tools.offset", json!({ "distance": 0 })).is_err());
    }

    #[test]
    fn offset_both_and_open_shapes() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 20.0, 20.0, 0);
        select(&mut app, &[r]);
        let out = run(&mut app, "tools.offset", json!({ "distance": 1, "side": "both" })).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 2);
        let line = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 50 L30 50" })).unwrap();
        select(&mut app, &[ShapeId(line["id"].as_u64().unwrap())]);
        assert!(run(&mut app, "tools.offset", json!({ "distance": 1 })).is_err(), "open shapes are skipped unless asked for");
        let out = run(&mut app, "tools.offset", json!({ "distance": 1, "open_shapes": true })).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn booleans() {
        let mut app = app();
        let a = add_rect(&mut app, 0.0, 0.0, 20.0, 20.0, 1);
        let b = add_rect(&mut app, 10.0, 10.0, 20.0, 20.0, 2);
        select(&mut app, &[a, b]);
        run(&mut app, "tools.union", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 1);
        assert!((area(&app) - 700.0).abs() < 0.5);
        assert_eq!(app.editor.doc.iter().next().unwrap().layer, 1, "the bottom shape's layer");
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 2);
        select(&mut app, &[a, b]);
        run(&mut app, "tools.intersection", Value::Null).unwrap();
        assert!((area(&app) - 100.0).abs() < 0.5);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        select(&mut app, &[a, b]);
        run(&mut app, "tools.difference", Value::Null).unwrap();
        assert!((area(&app) - 300.0).abs() < 0.5);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        select(&mut app, &[a, b]);
        run(&mut app, "tools.weld", Value::Null).unwrap();
        assert!((area(&app) - 700.0).abs() < 0.5);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        let out = run(&mut app, "tools.boolean_assistant", json!({ "op": "difference", "a": [b.0], "b": [a.0], "keep_originals": true })).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1);
        assert_eq!(app.editor.doc.len(), 3);
        assert!(run(&mut app, "tools.union", Value::Null).is_err() || !app.editor.selection.is_empty());
    }

    #[test]
    fn cut_shapes_splits_overlaps() {
        let mut app = app();
        let a = add_rect(&mut app, 0.0, 0.0, 20.0, 20.0, 1);
        let b = add_rect(&mut app, 10.0, 10.0, 20.0, 20.0, 1);
        select(&mut app, &[a, b]);
        run(&mut app, "tools.cut_shapes", Value::Null).unwrap();
        assert!(app.editor.doc.len() >= 3);
        assert!((area(&app) - 700.0).abs() < 0.5);
    }

    #[test]
    fn convert_close_join_optimize_duplicates() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        let e = run(&mut app, "shape.add_ellipse", json!({ "x": 20, "y": 0, "width": 10, "height": 10 })).unwrap();
        select(&mut app, &[r, ShapeId(e["id"].as_u64().unwrap())]);
        let out = run(&mut app, "edit.convert_path", Value::Null).unwrap();
        assert_eq!(out["converted"], 2);
        assert!(app.editor.doc.iter().all(|s| matches!(s.kind, ShapeKind::Path { .. })));
        // Two open halves that meet.
        let a = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 20 L10 20 L10 30" })).unwrap();
        let b = run(&mut app, "shape.add_path", json!({ "svg_path": "M10 30 L0 30 L0 20.05" })).unwrap();
        let ids = [ShapeId(a["id"].as_u64().unwrap()), ShapeId(b["id"].as_u64().unwrap())];
        select(&mut app, &ids);
        run(&mut app, "edit.auto_join", json!({ "tolerance": 0.1 })).unwrap();
        let joined = app.editor.doc.iter().last().unwrap();
        assert!(joined.is_closed() || gpath::contours(&joined.outline()).len() == 1);
        let p = app.editor.selected();
        assert_eq!(p.len(), 1);
        // Close a lone open path.
        let o = run(&mut app, "shape.add_path", json!({ "svg_path": "M50 0 L60 0 L60 10" })).unwrap();
        select(&mut app, &[ShapeId(o["id"].as_u64().unwrap())]);
        run(&mut app, "edit.close_path", Value::Null).unwrap();
        assert!(app.editor.doc.iter().last().unwrap().is_closed());
        // Duplicates.
        let d1 = add_rect(&mut app, 100.0, 0.0, 10.0, 10.0, 5);
        let d2 = add_rect(&mut app, 100.0, 0.0, 10.0, 10.0, 5);
        select(&mut app, &[d1, d2]);
        let before = app.editor.doc.len();
        let out = run(&mut app, "edit.delete_duplicates", Value::Null).unwrap();
        assert_eq!(out["removed"], 1);
        assert_eq!(app.editor.doc.len(), before - 1);
        // Optimize a path with redundant nodes on a line.
        let l = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 80 L5 80 L10 80 L15 80 L20 80" })).unwrap();
        select(&mut app, &[ShapeId(l["id"].as_u64().unwrap())]);
        let out = run(&mut app, "edit.optimize", Value::Null).unwrap();
        assert!(out["nodes_after"].as_u64().unwrap() < out["nodes_before"].as_u64().unwrap(), "{out}");
    }

    #[test]
    fn break_apart_and_auto_group() {
        let mut app = app();
        let c = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 0 L20 0 L20 20 L0 20 Z M5 5 L15 5 L15 15 L5 15 Z" })).unwrap();
        select(&mut app, &[ShapeId(c["id"].as_u64().unwrap())]);
        run(&mut app, "arrange.break_apart", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 2);
        run(&mut app, "edit.select_all", Value::Null).unwrap();
        let out = run(&mut app, "arrange.auto_group", Value::Null).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1);
        assert_eq!(app.editor.doc.len(), 1);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 2);
    }

    #[test]
    fn slots_resize_and_list() {
        let mut app = app();
        // A 40 x 20 plate with a 4 wide, 8 deep notch cut into the top edge.
        let p = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 0 L40 0 L40 20 L22 20 L22 12 L18 12 L18 20 L0 20 Z" })).unwrap();
        select(&mut app, &[ShapeId(p["id"].as_u64().unwrap())]);
        let out = run(&mut app, "tools.resize_slots", json!({})).unwrap();
        assert_eq!(out["slots"].as_array().unwrap().len(), 1, "{out}");
        run(&mut app, "tools.resize_slots", json!({ "new_width": 6 })).unwrap();
        let bb = app.editor.doc.iter().next().unwrap().bbox();
        assert!((bb.width() - 40.0).abs() < 0.01);
        let now = run(&mut app, "tools.resize_slots", json!({})).unwrap();
        assert!((now["slots"][0]["width"].as_f64().unwrap() - 6.0).abs() < 0.01, "{now}");
    }

    #[test]
    fn select_smaller_warp_deform_round() {
        let mut app = app();
        let big = add_rect(&mut app, 0.0, 0.0, 50.0, 50.0, 0);
        let small = add_rect(&mut app, 60.0, 0.0, 10.0, 10.0, 0);
        let _wide = add_rect(&mut app, 80.0, 0.0, 60.0, 5.0, 0);
        select(&mut app, &[big]);
        run(&mut app, "tools.select_smaller", Value::Null).unwrap();
        assert_eq!(app.editor.selected(), vec![small]);
        select(&mut app, &[small]);
        run(&mut app, "tools.round_corners", json!({ "radius": 2 })).unwrap();
        let s = app.editor.doc.get(small).unwrap();
        assert!(matches!(s.kind, ShapeKind::Path { .. }));
        assert!(s.outline().elements().len() > 5, "rounded corners add segments");
        run(&mut app, "edit.undo", Value::Null).unwrap();
        select(&mut app, &[small]);
        run(&mut app, "tools.warp", json!({ "corners": [[60, 0], [70, 0], [75, 10], [60, 10]] })).unwrap();
        assert!((app.editor.doc.get(small).unwrap().bbox().x1 - 75.0).abs() < 0.05);
        assert!(run(&mut app, "tools.warp", json!({ "corners": [[0, 0]] })).is_err());
        let mut grid = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                grid.push(json!([60.0 + j as f64 * 5.0, 10.0 * i as f64 / 3.0 + 1.0]));
            }
        }
        run(&mut app, "tools.deform", json!({ "grid": grid })).unwrap();
        assert!(app.editor.doc.get(small).unwrap().bbox().y0 > 0.5);
        // Points far out of reach are refused at once instead of bending the shapes towards them.
        let steps = app.editor.history.len();
        let mut far = grid.clone();
        far[8] = json!([1e150, 1e300]);
        assert!(run(&mut app, "tools.deform", json!({ "grid": far })).is_err());
        assert!(run(&mut app, "tools.warp", json!({ "corners": [[60, 0], [1e12, 0], [75, 10], [60, 10]] })).is_err());
        assert_eq!(app.editor.history.len(), steps);
    }

    #[test]
    fn mirror_two_point_rubber_band() {
        let mut app = app();
        let r = add_rect(&mut app, 10.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "arrange.mirror", json!({ "a": [0, 0], "b": [0, 10] })).unwrap();
        assert!((app.editor.doc.get(r).unwrap().bbox().x1 + 10.0).abs() < 1e-6);
        // The selected straight line is the axis.
        let line = run(&mut app, "shape.add_path", json!({ "svg_path": "M50 0 L50 10" })).unwrap();
        let l = ShapeId(line["id"].as_u64().unwrap());
        let r2 = add_rect(&mut app, 60.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r2, l]);
        run(&mut app, "arrange.mirror", json!({ "copy": true })).unwrap();
        assert_eq!(app.editor.doc.len(), 4);
        assert!(app.editor.doc.iter().any(|s| (s.bbox().x0 - 30.0).abs() < 1e-6 && (s.bbox().x1 - 40.0).abs() < 1e-6));
        let r3 = add_rect(&mut app, 100.0, 100.0, 10.0, 0.0001 + 10.0, 0);
        select(&mut app, &[r3]);
        run(
            &mut app,
            "arrange.two_point",
            json!({ "from_a": [100, 100], "from_b": [110, 100], "to_a": [100, 100], "to_b": [100, 120], "uniform": true }),
        )
        .unwrap();
        let bb = app.editor.doc.get(r3).unwrap().bbox();
        assert!((bb.height() - 20.0).abs() < 0.01, "{bb:?}");
        select(&mut app, &[r2, r3]);
        let out = run(&mut app, "arrange.rubber_band", Value::Null).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn schemas_are_json_and_commands_registered() {
        for id in ["tools.offset", "tools.boolean_assistant", "tools.resize_slots", "tools.warp", "arrange.mirror", "edit.auto_join"] {
            let s = args_schema(id).unwrap_or_else(|| panic!("{id}"));
            assert!(serde_json::from_str::<Value>(s).is_ok(), "{id}");
            assert!(crate::commands::lookup(id).is_some());
            assert!(works(id));
        }
    }

    #[test]
    fn every_edit_is_one_undo_step_and_undo_restores() {
        let cases: Vec<(&str, Value)> = vec![
            ("edit.convert_path", json!({})),
            ("tools.offset", json!({ "distance": 1 })),
            ("tools.weld", json!({})),
            ("tools.union", json!({})),
            ("tools.difference", json!({})),
            ("tools.intersection", json!({})),
            ("tools.cut_shapes", json!({})),
            ("tools.round_corners", json!({ "radius": 1 })),
            ("arrange.mirror", json!({ "a": [0, 0], "b": [0, 1] })),
            ("arrange.rubber_band", json!({})),
            ("arrange.grid_array", json!({ "rows": 2, "cols": 2, "virtual": true })),
            ("arrange.circular_array", json!({ "cx": 0, "cy": 0, "count": 3 })),
            ("arrange.nest", json!({})),
            ("arrange.dock_left", json!({})),
            ("tabs.add", json!({ "shape": 1, "x": 5, "y": 0 })),
            ("shape.set_start_point", json!({ "x": 20, "y": 20 })),
        ];
        for (id, args) in cases {
            let mut app = app();
            let a = add_rect(&mut app, 10.0, 10.0, 20.0, 20.0, 1);
            let b = add_rect(&mut app, 20.0, 20.0, 20.0, 20.0, 1);
            select(&mut app, &[a, b]);
            let before = app.editor.doc.clone();
            let steps = app.editor.history.len();
            let r = run(&mut app, id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(app.editor.history.len(), steps + 1, "{id} must be exactly one undo step: {r}");
            run(&mut app, "edit.undo", Value::Null).unwrap();
            assert_eq!(app.editor.doc, before, "{id}: undo restores the document");
            run(&mut app, "edit.redo", Value::Null).unwrap();
        }
    }

    #[test]
    fn every_command_of_this_area_is_registered_and_described() {
        for id in [
            "edit.convert_path",
            "edit.close_path",
            "edit.close_tolerance",
            "edit.auto_join",
            "edit.optimize",
            "edit.delete_duplicates",
            "edit.delete_overlapping",
            "shape.remove_rounded_corners",
            "tools.offset",
            "tools.weld",
            "tools.union",
            "tools.difference",
            "tools.intersection",
            "tools.boolean_assistant",
            "tools.cut_shapes",
            "tools.resize_slots",
            "tools.select_smaller",
            "tools.warp",
            "tools.deform",
            "tools.round_corners",
            "tools.start_points",
            "tools.nodes",
            "tools.tabs",
            "tools.measure",
            "arrange.break_apart",
            "arrange.auto_group",
            "arrange.mirror",
            "arrange.two_point",
            "arrange.nest",
            "arrange.dock_left",
            "arrange.dock_right",
            "arrange.dock_up",
            "arrange.dock_down",
            "arrange.rubber_band",
            "arrange.grid_array",
            "arrange.circular_array",
            "arrange.copy_along_path",
            "arrange.array_edit",
            "arrange.array_add",
            "arrange.array_remove",
            "arrange.array_exit",
            "arrange.array_flatten",
            "arrange.array_duplicate",
            "arrange.alignment_helper",
            "arrange.move_selection",
            "node.edit",
            "tabs.add",
            "tabs.remove",
            "tabs.clear",
            "tabs.auto",
            "shape.set_corner_radius",
            "shape.add_line",
            "shape.set_start_point",
            "shape.reverse_direction",
            "measure.selection",
            "measure.points",
            "measure.arc",
        ] {
            let info = crate::commands::describe(id).unwrap_or_else(|| panic!("{id} is not registered"));
            assert!(info.implemented, "{id} should be implemented");
            assert!(!info.hardware, "{id}");
            assert!(!info.checklist.is_empty(), "{id} names checklist items");
        }
        assert!(crate::commands::describe("tools.offset").unwrap().args_schema["properties"]["distance"].is_object());
    }
}
