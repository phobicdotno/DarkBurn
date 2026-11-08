//! Node editing, tabs, start points, rectangle corners, drawing lines that join existing ones,
//! and measuring. Part of the shape-editing commands; see `shape_ops.rs`.

use darkburn_core::kurbo::{BezPath, Point, Vec2};
use darkburn_core::{Document, Editor, PathDirection, Shape, ShapeId, ShapeKind};
use darkburn_geom::cleanup;
use darkburn_geom::node::{self, NodeRef};
use darkburn_geom::path as gpath;
use darkburn_geom::query;
use darkburn_geom::tabs::{self, Gap, TabOptions};
use serde_json::{Value, json};

use crate::shape_ops::{Env, R, flag, num, ok, point, set_shape_path, shape_path, word};

pub fn works(id: &str) -> bool {
    matches!(
        id,
        "tools.nodes"
            | "tools.tabs"
            | "tools.measure"
            | "tools.start_points"
            | "node.edit"
            | "tabs.add"
            | "tabs.remove"
            | "tabs.clear"
            | "tabs.auto"
            | "shape.set_corner_radius"
            | "shape.remove_rounded_corners"
            | "shape.add_line"
            | "shape.set_start_point"
            | "shape.reverse_direction"
            | "shape.set_direction"
            | "shape.clear_start_point"
            | "measure.selection"
            | "measure.points"
            | "measure.arc"
    )
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "tools.nodes" | "node.edit" => {
            &["TOOLS-06", "NODE-01", "NODE-02", "NODE-03", "NODE-04", "NODE-05", "NODE-06", "NODE-07", "NODE-08", "NODE-09", "SHAPE-18"]
        }
        "tools.tabs" | "tabs.add" | "tabs.remove" | "tabs.clear" | "tabs.auto" => &["TOOLS-07"],
        "tools.measure" | "measure.selection" | "measure.points" | "measure.arc" => &["TOOLS-10", "DRAW-18", "DRAW-19"],
        "tools.start_points" | "shape.set_start_point" | "shape.reverse_direction" | "shape.set_direction" | "shape.clear_start_point" => {
            &["NODE-10", "NODE-11", "NODE-12", "TB-11", "PLAN-13", "LAYER-09"]
        }
        "shape.set_corner_radius" => &["DRAW-04"],
        "shape.remove_rounded_corners" => &["DRAW-05"],
        "shape.add_line" => &["DRAW-03"],
        _ => &[],
    }
}

pub fn args_schema(id: &str) -> Option<&'static str> {
    Some(match id {
        "node.edit" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer","description":"the selected shape when missing"},"op":{"enum":["insert","delete","delete_segment","break","to_line","to_curve","corner","smooth","move","align","trim","extend"]},"subpath":{"type":"integer"},"node":{"type":"integer"},"seg":{"type":"integer"},"t":{"type":"number","description":"0 to 1 along the segment for insert"},"dx":{"type":"number"},"dy":{"type":"number"},"nodes":{"type":"array","description":"[subpath, node] pairs for align"},"align":{"enum":["left","right","top","bottom","center_h","center_v"]},"at":{"type":"array","description":"[x, y] for trim"},"max_dist":{"type":"number"}},"required":["op"]}"#
        }
        "tabs.add" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"}},"required":["x","y"]}"#
        }
        "tabs.remove" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"},"radius":{"type":"number"}},"required":["x","y"]}"#
        }
        "tabs.clear" => r#"{"type":"object","properties":{"shape":{"type":"integer"}}}"#,
        "tabs.auto" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"force":{"type":"boolean","description":"even when the layer's tabs are off"}}}"#
        }
        "shape.set_corner_radius" => {
            r#"{"type":"object","properties":{"radius":{"type":"number","description":"mm; negative rounds inwards"}},"required":["radius"]}"#
        }
        "shape.add_line" => {
            r#"{"type":"object","properties":{"points":{"type":"array","description":"[x, y] points"},"closed":{"type":"boolean"},"connect":{"type":"boolean","description":"join to an open line on the same layer that ends there"},"layer":{"type":"integer"},"tolerance":{"type":"number"}},"required":["points"]}"#
        }
        "shape.set_start_point" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"},"all":{"type":"boolean","description":"every sub-path, not just the nearest"}},"required":["x","y"]}"#
        }
        "shape.reverse_direction" | "shape.clear_start_point" => r#"{"type":"object","properties":{"shape":{"type":"integer"}}}"#,
        "shape.set_direction" => {
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"direction":{"enum":["none","as_drawn","reversed","clockwise","counter_clockwise"]}},"required":["direction"]}"#
        }
        "measure.selection" => r#"{"type":"object","properties":{}}"#,
        "measure.points" => r#"{"type":"object","properties":{"a":{"type":"array"},"b":{"type":"array"}},"required":["a","b"]}"#,
        "measure.arc" => {
            r#"{"type":"object","properties":{"a":{"type":"array"},"b":{"type":"array"},"c":{"type":"array","description":"three points on the arc"}},"required":["a","b","c"]}"#
        }
        _ => return None,
    })
}

fn target(e: &Editor, a: &Value) -> Result<ShapeId, String> {
    if let Some(id) = a.get("shape").and_then(Value::as_u64) {
        return Ok(ShapeId(id));
    }
    match e.selected().as_slice() {
        [one] => Ok(*one),
        [] => Err("nothing is selected".into()),
        _ => Err("select one shape or pass shape".into()),
    }
}

fn targets(e: &Editor, a: &Value) -> Result<Vec<ShapeId>, String> {
    if a.get("shape").is_some() {
        return target(e, a).map(|t| vec![t]);
    }
    let ids = e.selected();
    if ids.is_empty() { Err("nothing is selected".into()) } else { Ok(ids) }
}

fn node_ref(a: &Value) -> NodeRef {
    NodeRef::new(num(a, "subpath", 0.0) as usize, num(a, "node", 0.0) as usize)
}

fn node_list(a: &Value) -> Vec<NodeRef> {
    a.get("nodes")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(|n| Some(NodeRef::new(n.get(0)?.as_u64()? as usize, n.get(1)?.as_u64()? as usize))).collect())
        .unwrap_or_default()
}

/// Outlines of every plain shape except `id`, for trimming and extending against.
fn others(doc: &Document, id: ShapeId) -> Vec<BezPath> {
    doc.iter().filter(|s| s.id != id).flat_map(|s| s.leaves()).filter(|l| !l.is_image()).map(|l| l.outline()).collect()
}

fn node_edit(e: &mut Editor, a: &Value) -> R {
    let id = target(e, a)?;
    let path = shape_path(&e.doc, id).ok_or("node editing needs a plain shape (ungroup first)")?;
    let (sub, seg) = (num(a, "subpath", 0.0) as usize, num(a, "seg", 0.0) as usize);
    let op = word(a, "op", "");
    let result = match op {
        "insert" => node::insert_node(&path, sub, seg, num(a, "t", 0.5)),
        "delete" => node::delete_node(&path, node_ref(a)),
        "delete_segment" => node::delete_segment(&path, sub, seg),
        "break" => node::break_at_node(&path, node_ref(a)),
        "to_line" => node::segment_to_line(&path, sub, seg),
        "to_curve" => node::segment_to_curve(&path, sub, seg),
        "corner" => node::set_node_corner(&path, node_ref(a)),
        "smooth" => node::set_node_smooth(&path, node_ref(a)),
        "move" => node::move_node(&path, node_ref(a), Vec2::new(num(a, "dx", 0.0), num(a, "dy", 0.0))),
        "align" => {
            let how = match word(a, "align", "left") {
                "left" => node::Align::Left,
                "right" => node::Align::Right,
                "top" => node::Align::Top,
                "bottom" => node::Align::Bottom,
                "center_h" => node::Align::CenterH,
                "center_v" => node::Align::CenterV,
                other => return Err(format!("unknown alignment {other}")),
            };
            node::align_nodes(&path, &node_list(a), how)
        }
        "trim" => {
            let at = point(a, "at").ok_or("trim needs the point at")?;
            node::trim_at_intersection(&path, &others(&e.doc, id), at)
        }
        "extend" => node::extend_to_intersection(&path, &others(&e.doc, id), node_ref(a), num(a, "max_dist", 1000.0)),
        other => return Err(format!("unknown node operation {other:?}")),
    }
    .map_err(|err| err.to_string())?;
    let done = e.edit("Edit nodes", |d, sel| {
        sel.set([id]);
        set_shape_path(d, id, result)
    });
    ok(done, &[id])
}

// ---------------------------------------------------------------- tabs

fn tabs_add(e: &mut Editor, a: &Value) -> R {
    let id = target(e, a)?;
    let at = Point::new(num(a, "x", 0.0), num(a, "y", 0.0));
    let s = e.doc.get(id).ok_or("no such shape")?;
    if s.is_group() || s.is_image() {
        return Err("tabs go on plain shapes".into());
    }
    let hit = query::nearest_point(&s.outline(), at).ok_or("the shape has no outline")?;
    let local = s.transform.inverse() * hit.point;
    let done = e.edit("Add tab", |d, sel| {
        sel.set([id]);
        match d.get_mut(id) {
            Some(s) if !s.tabs.iter().any(|t| Point::new(t[0], t[1]).distance(local) < 1e-3) => {
                s.tabs.push([local.x, local.y]);
                true
            }
            _ => false,
        }
    });
    Ok(json!({ "changed": done, "x": hit.point.x, "y": hit.point.y }))
}

fn tabs_remove(e: &mut Editor, a: &Value) -> R {
    let id = target(e, a)?;
    let at = Point::new(num(a, "x", 0.0), num(a, "y", 0.0));
    let radius = num(a, "radius", 1.0).max(1e-6);
    let done = e.edit("Remove tab", |d, _| {
        let Some(s) = d.get_mut(id) else { return false };
        let t = s.transform;
        let before = s.tabs.len();
        s.tabs.retain(|p| (t * Point::new(p[0], p[1])).distance(at) > radius);
        s.tabs.len() != before
    });
    ok(done, &[id])
}

fn tabs_clear(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let done = e.edit("Clear tabs", |d, _| {
        let mut any = false;
        for id in &ids {
            if let Some(s) = d.get(*id)
                && !s.tabs.is_empty()
                && let Some(s) = d.get_mut(*id)
            {
                s.tabs.clear();
                any = true;
            }
        }
        any
    });
    ok(done, &ids)
}

/// The gaps the layer's automatic tab settings leave in shape `id` (none when its layer's tabs
/// are off, unless `force`).
pub fn auto_gaps(doc: &Document, id: ShapeId, force: bool) -> Vec<Gap> {
    let Some(s) = doc.get(id) else { return Vec::new() };
    let t = doc.layer(s.layer).main().tabs;
    if (!t.enabled && !force) || !t.auto_spacing {
        return Vec::new();
    }
    let opts = TabOptions {
        count: t.min_count.max(1) as usize,
        spacing: Some(t.spacing.max(0.1)),
        min_count: t.min_count as usize,
        max_count: t.max_count.max(1) as usize,
        size: t.size,
        skip_shorter_than: None,
        offset_from_start: 0.0,
    };
    let subs = gpath::subpaths(&s.outline());
    let tree = query::containment(&subs, 1e-6);
    let mut out = Vec::new();
    for (i, sub) in subs.iter().enumerate() {
        if t.skip_inner && tree.depth.get(i).copied().unwrap_or(0) > 0 {
            continue;
        }
        out.extend(tabs::place_tabs(sub, &opts).gaps);
    }
    out
}

fn tabs_auto(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let force = flag(a, "force", false);
    let list: Vec<Value> = ids
        .iter()
        .flat_map(|id| {
            auto_gaps(&e.doc, *id, force)
                .into_iter()
                .map(move |g| json!({ "shape": id.0, "from": [g.from.x, g.from.y], "to": [g.to.x, g.to.y], "length": g.len }))
        })
        .collect();
    Ok(json!({ "changed": false, "gaps": list }))
}

// ---------------------------------------------------------------- start points, corners

/// The shape's outline as it will be cut: with its forced start point and direction applied.
/// Used for the start and end markers of the Start Points tool.
pub fn marker_path(doc: &Document, id: ShapeId) -> Option<BezPath> {
    let s = doc.get(id)?;
    let mut path = shape_path(doc, id)?;
    if let Some(st) = s.start {
        path = gpath::set_start_point(&path, s.transform * Point::new(st[0], st[1]), false);
    }
    path = match s.direction {
        Some(PathDirection::Reversed) => gpath::reverse(&path),
        Some(PathDirection::Clockwise) => gpath::set_direction(&path, gpath::Direction::Clockwise),
        Some(PathDirection::CounterClockwise) => gpath::set_direction(&path, gpath::Direction::CounterClockwise),
        _ => path,
    };
    Some(path)
}

fn set_start(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let p = Point::new(num(a, "x", 0.0), num(a, "y", 0.0));
    let all = flag(a, "all", false);
    let done = e.edit("Set start point", |d, _| {
        let mut any = false;
        for id in &ids {
            let Some(path) = shape_path(d, *id) else { continue };
            let Some(at) = query::nearest_point(&path, p).map(|h| h.point) else { continue };
            let is_path = d.get(*id).is_some_and(|s| matches!(s.kind, ShapeKind::Path { .. }));
            if is_path {
                // Paths keep their own sub-path order: rotate the nearest sub-path (or all).
                let q = gpath::set_start_point(&path, p, all);
                if q != path {
                    any |= set_shape_path(d, *id, q);
                }
            }
            if let Some(s) = d.get_mut(*id) {
                let local = s.transform.inverse() * at;
                let new = Some([local.x, local.y]);
                if s.start != new {
                    s.start = new;
                    any = true;
                }
            }
        }
        any
    });
    ok(done, &ids)
}

fn reverse(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let done = e.edit("Reverse direction", |d, _| {
        let mut any = false;
        for id in &ids {
            let is_path = d.get(*id).is_some_and(|s| matches!(s.kind, ShapeKind::Path { .. }));
            if is_path && let Some(path) = shape_path(d, *id) {
                any |= set_shape_path(d, *id, gpath::reverse(&path));
                if let Some(s) = d.get_mut(*id) {
                    s.direction = None;
                }
            } else if let Some(s) = d.get_mut(*id)
                && !s.is_group()
                && !s.is_image()
            {
                s.direction = Some(if s.direction == Some(PathDirection::Reversed) { PathDirection::AsDrawn } else { PathDirection::Reversed });
                any = true;
            }
        }
        any
    });
    ok(done, &ids)
}

/// Force the cut direction (`as_drawn`, `reversed`, `clockwise`, `counter_clockwise`) or clear
/// the override (`none`) on the selected shapes. The geometry is not changed.
fn set_direction(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let dir = match word(a, "direction", "none") {
        "as_drawn" => Some(PathDirection::AsDrawn),
        "reversed" => Some(PathDirection::Reversed),
        "clockwise" => Some(PathDirection::Clockwise),
        "counter_clockwise" => Some(PathDirection::CounterClockwise),
        "none" => None,
        other => return Err(format!("unknown direction {other}")),
    };
    let done = e.edit("Set cut direction", |d, _| {
        let mut any = false;
        for id in &ids {
            if let Some(s) = d.get_mut(*id)
                && s.direction != dir
            {
                s.direction = dir;
                any = true;
            }
        }
        any
    });
    ok(done, &ids)
}

/// Clear the forced start point of the selected shapes.
fn clear_start(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let done = e.edit("Clear start point", |d, _| {
        let mut any = false;
        for id in &ids {
            if let Some(s) = d.get_mut(*id)
                && s.start.is_some()
            {
                s.start = None;
                any = true;
            }
        }
        any
    });
    ok(done, &ids)
}

fn for_each_rect(s: &mut Shape, f: &mut dyn FnMut(&mut f64, f64, f64) -> bool) -> usize {
    match &mut s.kind {
        ShapeKind::Rect { width, height, radius } => usize::from(f(radius, *width, *height)),
        ShapeKind::Group { children } => children.iter_mut().map(|c| for_each_rect(c, f)).sum(),
        _ => 0,
    }
}

/// Remove tangent-arc corners from path shapes (also inside groups). Returns how many changed.
fn unround_paths(s: &mut Shape) -> usize {
    match &mut s.kind {
        ShapeKind::Group { children } => children.iter_mut().map(unround_paths).sum(),
        ShapeKind::Path { path } => {
            let new = darkburn_geom::unfillet::remove_rounded_corners(path, 1e-3);
            if new != *path {
                *path = new;
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn set_corner_radius(e: &mut Editor, a: &Value, remove: bool) -> R {
    let ids = e.selected();
    if ids.is_empty() {
        return Err("nothing is selected".into());
    }
    let r = if remove { 0.0 } else { num(a, "radius", 0.0) };
    let mut n = 0;
    let mut paths = 0;
    let done = e.edit(if remove { "Remove rounded corners" } else { "Corner radius" }, |d, _| {
        for id in &ids {
            if let Some(s) = d.get(*id)
                && let Some(s) = d.get_mut(s.id)
            {
                n += for_each_rect(s, &mut |radius, w, h| {
                    let limit = w.min(h) / 2.0;
                    let new = r.clamp(-limit, limit);
                    let changed = (*radius - new).abs() > 1e-12;
                    *radius = new;
                    changed
                });
                if remove {
                    paths += unround_paths(s);
                }
            }
        }
        n + paths > 0
    });
    Ok(json!({ "changed": done, "rectangles": n, "paths": paths }))
}

// ---------------------------------------------------------------- lines

/// Add `path` as a new line on `layer`; when `connect` and one of its ends meets the end of an
/// open line on the same layer within `tol`, extend that line instead. Returns the shape and
/// whether it joined an old one. Call inside an edit.
pub fn add_line_in(d: &mut Document, path: BezPath, layer: usize, connect: bool, tol: f64) -> (ShapeId, bool) {
    if connect && !path.elements().is_empty() {
        let ends = {
            let cs = gpath::contours(&path);
            cs.first().filter(|c| cs.len() == 1 && !c.closed).map(|c| (c.start().unwrap_or_default(), c.end().unwrap_or_default()))
        };
        if let Some((ns, ne)) = ends {
            let cand: Option<(ShapeId, BezPath)> =
                d.iter().rev().filter(|s| s.layer == layer && !s.locked && matches!(s.kind, ShapeKind::Path { .. })).find_map(|s| {
                    let p = s.outline();
                    let cs = gpath::contours(&p);
                    let c = cs.first()?;
                    if cs.len() != 1 || c.closed {
                        return None;
                    }
                    let (cs0, ce) = (c.start()?, c.end()?);
                    let near = [cs0.distance(ns), cs0.distance(ne), ce.distance(ns), ce.distance(ne)].into_iter().fold(f64::INFINITY, f64::min);
                    (near <= tol).then_some((s.id, p))
                });
            if let Some((id, old)) = cand
                && let Some(joined) = cleanup::auto_join(&[old, path.clone()], tol.max(1e-6), &mut |_, _| true)
                && joined.len() == 1
            {
                set_shape_path(d, id, joined.into_iter().next().unwrap_or_default());
                return (id, true);
            }
        }
    }
    (d.add(Shape::path(path, layer)), false)
}

fn add_line(e: &mut Editor, a: &Value, _env: &Env) -> R {
    let pts: Vec<Point> = a
        .get("points")
        .and_then(Value::as_array)
        .ok_or("points must be a list of [x, y]")?
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some(Point::new(p.first()?.as_f64()?, p.get(1)?.as_f64()?))
        })
        .collect();
    if pts.len() < 2 {
        return Err("a line needs at least two points".into());
    }
    let mut path = BezPath::new();
    path.move_to(pts[0]);
    for p in &pts[1..] {
        path.line_to(*p);
    }
    let closed = flag(a, "closed", false) && pts.len() >= 3;
    if closed {
        path.close_path();
    }
    let layer = num(a, "layer", e.current_layer as f64).clamp(0.0, 31.0) as usize;
    let connect = flag(a, "connect", true) && !closed;
    let tol = num(a, "tolerance", 0.05).max(1e-6);
    let mut result = (ShapeId(0), false);
    e.edit("Draw line", |d, sel| {
        result = add_line_in(d, path, layer, connect, tol);
        sel.set([result.0]);
        true
    });
    Ok(json!({ "changed": true, "id": result.0.0, "joined": result.1 }))
}

// ---------------------------------------------------------------- measuring

fn measure_selection(e: &mut Editor, _a: &Value) -> R {
    let ids = e.selected();
    if ids.is_empty() {
        return Err("nothing is selected".into());
    }
    let (mut length, mut area, mut nodes) = (0.0, 0.0, 0);
    for id in &ids {
        for part in crate::shape_ops::parts_of(&e.doc, *id) {
            length += gpath::length(&part.path);
            area += gpath::area(&part.path, darkburn_geom::FillRule::EvenOdd, 0.01).abs();
            nodes += cleanup::node_count(&part.path);
        }
    }
    let bb = e.doc.bbox_of(&ids).ok_or("nothing to measure")?;
    Ok(
        json!({ "count": ids.len(), "x": bb.x0, "y": bb.y0, "width": bb.width(), "height": bb.height(), "length": length, "area": area, "nodes": nodes }),
    )
}

fn measure_points(a: &Value) -> R {
    let (Some(p), Some(q)) = (point(a, "a"), point(a, "b")) else { return Err("a and b are needed ([x, y])".into()) };
    let d = q - p;
    Ok(json!({ "distance": d.hypot(), "dx": d.x, "dy": d.y, "angle": d.y.atan2(d.x).to_degrees() }))
}

fn measure_arc(a: &Value) -> R {
    let (Some(p), Some(q), Some(r)) = (point(a, "a"), point(a, "b"), point(a, "c")) else { return Err("a, b and c are needed ([x, y])".into()) };
    let m = query::arc_through(p, q, r).ok_or("the three points are on a line")?;
    Ok(json!({ "radius": m.radius, "sweep_degrees": m.sweep.to_degrees(), "length": m.length, "centre": [m.center.x, m.center.y] }))
}

pub fn exec(e: &mut Editor, env: &Env, id: &str, a: &Value) -> Option<R> {
    Some(match id {
        "node.edit" => node_edit(e, a),
        "tabs.add" => tabs_add(e, a),
        "tabs.remove" => tabs_remove(e, a),
        "tabs.clear" => tabs_clear(e, a),
        "tabs.auto" => tabs_auto(e, a),
        "shape.set_corner_radius" => set_corner_radius(e, a, false),
        "shape.remove_rounded_corners" => set_corner_radius(e, a, true),
        "shape.add_line" => add_line(e, a, env),
        "shape.set_start_point" => set_start(e, a),
        "shape.set_direction" => set_direction(e, a),
        "shape.clear_start_point" => clear_start(e, a),
        "shape.reverse_direction" => reverse(e, a),
        "measure.selection" => measure_selection(e, a),
        "measure.points" => measure_points(a),
        "measure.arc" => measure_arc(a),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_ops::tests::{add_rect, app, run, select};

    #[test]
    fn node_edit_insert_move_delete() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "node.edit", json!({ "op": "insert", "subpath": 0, "seg": 0, "t": 0.5 })).unwrap();
        let n = |app: &crate::shell::DarkBurn| cleanup::node_count(&app.editor.doc.get(r).unwrap().outline());
        assert_eq!(n(&app), 5);
        run(&mut app, "node.edit", json!({ "op": "move", "subpath": 0, "node": 1, "dx": 0, "dy": -3 })).unwrap();
        assert!((app.editor.doc.get(r).unwrap().bbox().y0 + 3.0).abs() < 1e-6);
        run(&mut app, "node.edit", json!({ "op": "delete", "subpath": 0, "node": 1 })).unwrap();
        assert_eq!(n(&app), 4);
        run(&mut app, "node.edit", json!({ "op": "to_curve", "subpath": 0, "seg": 0 })).unwrap();
        run(&mut app, "node.edit", json!({ "op": "smooth", "subpath": 0, "node": 1 })).unwrap();
        run(&mut app, "node.edit", json!({ "op": "align", "nodes": [[0, 0], [0, 1]], "align": "left" })).unwrap();
        assert!(run(&mut app, "node.edit", json!({ "op": "bogus" })).is_err());
        assert!(run(&mut app, "node.edit", json!({ "op": "delete", "subpath": 9, "node": 0 })).is_err());
        // Break then start point and direction.
        run(&mut app, "shape.set_start_point", json!({ "x": 10, "y": 10 })).unwrap();
        run(&mut app, "shape.reverse_direction", json!({})).unwrap();
        assert!(app.editor.history.len() >= 6);
    }

    #[test]
    fn start_point_and_direction_are_overrides_that_keep_the_shape_editable() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "shape.set_start_point", json!({ "x": 10.2, "y": 5.0 })).unwrap();
        let sh = app.editor.doc.get(r).unwrap();
        assert!(matches!(sh.kind, ShapeKind::Rect { .. }), "still a rectangle");
        let st = sh.start.expect("start recorded");
        assert!((st[0] - 5.0).abs() < 1e-6 || (st[0] - 10.0).abs() < 1e-6 || (st[0] + 5.0).abs() < 1e-6, "{st:?}");
        let mp = marker_path(&app.editor.doc, r).unwrap();
        let first = gpath::start_point(&mp).unwrap();
        assert!(first.distance(Point::new(10.0, 5.0)) < 1e-6, "marker starts at the click: {first:?}");
        run(&mut app, "shape.reverse_direction", json!({})).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, Some(PathDirection::Reversed));
        run(&mut app, "shape.reverse_direction", json!({})).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, Some(PathDirection::AsDrawn));
        run(&mut app, "shape.set_direction", json!({ "direction": "clockwise" })).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, Some(PathDirection::Clockwise));
        assert!(run(&mut app, "shape.set_direction", json!({ "direction": "sideways" })).is_err());
        run(&mut app, "shape.set_direction", json!({ "direction": "none" })).unwrap();
        run(&mut app, "shape.clear_start_point", json!({})).unwrap();
        let sh = app.editor.doc.get(r).unwrap();
        assert!(sh.start.is_none() && sh.direction.is_none());
        // Undo restores the override.
        app.editor.undo();
        assert!(app.editor.doc.get(r).unwrap().start.is_some());
    }

    #[test]
    fn remove_rounded_corners_works_on_rounded_paths_too() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 20.0, 10.0, 0);
        select(&mut app, &[r]);
        // Round it, turn it into a path (so the corners are arcs), then take the rounding off.
        run(&mut app, "shape.set_corner_radius", json!({ "radius": 3 })).unwrap();
        run(&mut app, "edit.convert_path", Value::Null).unwrap();
        let before = cleanup::node_count(&app.editor.doc.get(r).unwrap().outline());
        assert!(before > 4);
        let out = run(&mut app, "shape.remove_rounded_corners", Value::Null).unwrap();
        assert_eq!(out["paths"], 1, "{out}");
        let after = app.editor.doc.get(r).unwrap().outline();
        assert_eq!(cleanup::node_count(&after), 4, "{:?}", after.elements());
        let bb = app.editor.doc.get(r).unwrap().bbox();
        assert!((bb.width() - 20.0).abs() < 1e-6 && (bb.height() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn manual_and_auto_tabs() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 100.0, 100.0, 4);
        select(&mut app, &[r]);
        let out = run(&mut app, "tabs.add", json!({ "x": 50.2, "y": 0.4 })).unwrap();
        assert!((out["y"].as_f64().unwrap()).abs() < 1e-6, "snapped to the outline: {out}");
        assert_eq!(app.editor.doc.get(r).unwrap().tabs.len(), 1);
        run(&mut app, "tabs.remove", json!({ "x": 50.0, "y": 0.0 })).unwrap();
        assert!(app.editor.doc.get(r).unwrap().tabs.is_empty());
        run(&mut app, "tabs.add", json!({ "x": 10.0, "y": 0.0 })).unwrap();
        run(&mut app, "tabs.clear", json!({})).unwrap();
        assert!(app.editor.doc.get(r).unwrap().tabs.is_empty());
        let none = run(&mut app, "tabs.auto", json!({})).unwrap();
        assert!(none["gaps"].as_array().unwrap().is_empty(), "layer tabs are off");
        app.editor.doc.layer_mut(4).main_mut().tabs.enabled = true;
        let gaps = run(&mut app, "tabs.auto", json!({})).unwrap();
        assert!(!gaps["gaps"].as_array().unwrap().is_empty(), "{gaps}");
        // Tabs survive a transform bake.
        run(&mut app, "tabs.add", json!({ "x": 50.0, "y": 0.0 })).unwrap();
        run(&mut app, "edit.convert_path", Value::Null).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().tabs.len(), 1);
    }

    #[test]
    fn rectangle_corners_and_lines_that_connect() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 20.0, 10.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "shape.set_corner_radius", json!({ "radius": -3 })).unwrap();
        assert!(matches!(app.editor.doc.get(r).unwrap().kind, ShapeKind::Rect { radius, .. } if (radius + 3.0).abs() < 1e-9));
        run(&mut app, "shape.set_corner_radius", json!({ "radius": 50 })).unwrap();
        assert!(
            matches!(app.editor.doc.get(r).unwrap().kind, ShapeKind::Rect { radius, .. } if (radius - 5.0).abs() < 1e-9),
            "limited to half the short side"
        );
        run(&mut app, "shape.remove_rounded_corners", Value::Null).unwrap();
        assert!(matches!(app.editor.doc.get(r).unwrap().kind, ShapeKind::Rect { radius, .. } if radius == 0.0));
        let a = run(&mut app, "shape.add_line", json!({ "points": [[100, 0], [110, 0]], "layer": 2 })).unwrap();
        let b = run(&mut app, "shape.add_line", json!({ "points": [[110, 0], [110, 10]], "layer": 2 })).unwrap();
        assert_eq!(b["joined"], true);
        assert_eq!(b["id"], a["id"]);
        let c = run(&mut app, "shape.add_line", json!({ "points": [[110, 0], [120, 0]], "layer": 3 })).unwrap();
        assert_eq!(c["joined"], false, "other layers are not joined");
    }

    #[test]
    fn measuring() {
        let mut app = app();
        let r = add_rect(&mut app, 5.0, 5.0, 20.0, 10.0, 0);
        select(&mut app, &[r]);
        let m = run(&mut app, "measure.selection", Value::Null).unwrap();
        assert!((m["area"].as_f64().unwrap() - 200.0).abs() < 0.1 && (m["length"].as_f64().unwrap() - 60.0).abs() < 0.1, "{m}");
        let p = run(&mut app, "measure.points", json!({ "a": [0, 0], "b": [3, 4] })).unwrap();
        assert!((p["distance"].as_f64().unwrap() - 5.0).abs() < 1e-9);
        let arc = run(&mut app, "measure.arc", json!({ "a": [10, 0], "b": [0, 10], "c": [-10, 0] })).unwrap();
        assert!(
            (arc["radius"].as_f64().unwrap() - 10.0).abs() < 1e-6 && (arc["sweep_degrees"].as_f64().unwrap().abs() - 180.0).abs() < 1e-3,
            "{arc}"
        );
    }
}
