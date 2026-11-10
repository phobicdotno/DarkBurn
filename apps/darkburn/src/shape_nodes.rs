//! Node editing, tabs, start points, rectangle corners, drawing lines that join existing ones,
//! and measuring. Part of the shape-editing commands; see `shape_ops.rs`.

use darkburn_core::kurbo::{BezPath, Point, Vec2};
use darkburn_core::{CutDirection, Document, Editor, Shape, ShapeId, ShapeKind};
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
            r#"{"type":"object","properties":{"shape":{"type":"integer"},"direction":{"enum":["auto","reversed","clockwise","counter_clockwise"]}},"required":["direction"]}"#
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

/// The plain shapes that carry the tabs and start points of `id`, each with its document
/// transform: the shape itself, or for a group every shape in it (see `Shape::leaves`). The
/// number is the shape's place among the leaves, for [`nth_leaf_mut`]. Pictures take neither,
/// and a virtual array is made again from its originals, so it has none of its own.
fn cut_leaves(doc: &Document, id: ShapeId) -> Vec<(usize, Shape)> {
    match doc.get(id) {
        Some(s) if s.array.is_none() => s.leaves().into_iter().enumerate().filter(|(_, l)| !l.is_image() && !l.is_group()).collect(),
        _ => Vec::new(),
    }
}

/// Leaf number `n` of `s`, in the order of `Shape::leaves`.
fn nth_leaf_mut<'a>(s: &'a mut Shape, n: &mut usize) -> Option<&'a mut Shape> {
    if s.is_group() {
        let ShapeKind::Group { children } = &mut s.kind else { return None };
        for c in children {
            if let Some(found) = nth_leaf_mut(c, n) {
                return Some(found);
            }
        }
        return None;
    }
    if *n == 0 {
        Some(s)
    } else {
        *n -= 1;
        None
    }
}

fn leaf_mut(doc: &mut Document, id: ShapeId, n: usize) -> Option<&mut Shape> {
    let mut n = n;
    nth_leaf_mut(doc.get_mut(id)?, &mut n)
}

/// Do `f` to every plain shape of `s` (itself, or the shapes in a group) with the transform of
/// what it sits in; returns whether any call said it changed something.
fn each_leaf_mut(s: &mut Shape, parent: darkburn_core::kurbo::Affine, f: &mut dyn FnMut(&mut Shape, darkburn_core::kurbo::Affine) -> bool) -> bool {
    if s.array.is_some() {
        return false;
    }
    let own = s.transform;
    match &mut s.kind {
        ShapeKind::Group { children } => children.iter_mut().fold(false, |any, c| each_leaf_mut(c, parent * own, f) | any),
        ShapeKind::Image(_) => false,
        _ => f(s, parent),
    }
}

/// The outline tabs and start points are placed on: the shape's own, or for a group the
/// outlines of the plain shapes in it.
pub fn cut_outline(doc: &Document, id: ShapeId) -> Option<BezPath> {
    let leaves = cut_leaves(doc, id);
    (!leaves.is_empty()).then(|| leaves.iter().flat_map(|(_, l)| l.outline()).collect())
}

/// Where the manual tabs of `id` are (document coordinates), those of shapes in a group too.
pub fn tab_points(doc: &Document, id: ShapeId) -> Vec<Point> {
    cut_leaves(doc, id).iter().flat_map(|(_, l)| l.tabs.iter().map(|t| l.transform * Point::new(t[0], t[1]))).collect()
}

fn tabs_add(e: &mut Editor, a: &Value) -> R {
    let id = target(e, a)?;
    let at = Point::new(num(a, "x", 0.0), num(a, "y", 0.0));
    e.doc.get(id).ok_or("no such shape")?;
    let leaves = cut_leaves(&e.doc, id);
    if leaves.is_empty() {
        return Err("tabs go on shapes with an outline, not on pictures or arrays".into());
    }
    // The tab goes on the outline nearest to the point: in a group, on that shape of it.
    let (n, leaf, hit) = leaves
        .iter()
        .filter_map(|(n, l)| query::nearest_point(&l.outline(), at).map(|h| (*n, l, h)))
        .min_by(|a, b| a.2.distance.total_cmp(&b.2.distance))
        .ok_or("the shape has no outline")?;
    let local = leaf.transform.inverse() * hit.point;
    let done = e.edit("Add tab", |d, sel| {
        sel.set([id]);
        match leaf_mut(d, id, n) {
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
        each_leaf_mut(s, darkburn_core::kurbo::Affine::IDENTITY, &mut |l, parent| {
            let t = parent * l.transform;
            let before = l.tabs.len();
            l.tabs.retain(|p| (t * Point::new(p[0], p[1])).distance(at) > radius);
            l.tabs.len() != before
        })
    });
    ok(done, &[id])
}

fn tabs_clear(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let done = e.edit("Clear tabs", |d, _| {
        let mut any = false;
        for id in &ids {
            if tab_points(d, *id).is_empty() {
                continue;
            }
            if let Some(s) = d.get_mut(*id) {
                any |= each_leaf_mut(s, darkburn_core::kurbo::Affine::IDENTITY, &mut |l, _| {
                    let had = !l.tabs.is_empty();
                    l.tabs.clear();
                    had
                });
            }
        }
        any
    });
    ok(done, &ids)
}

/// The gaps the layer's automatic tab settings leave in shape `id` (none when its layer's tabs
/// are off, unless `force`, and none on a shape with manual tabs: those replace them). For a
/// group: the gaps of every shape in it.
pub fn auto_gaps(doc: &Document, id: ShapeId, force: bool) -> Vec<Gap> {
    cut_leaves(doc, id).iter().flat_map(|(_, l)| leaf_auto_gaps(doc, l, force)).collect()
}

fn leaf_auto_gaps(doc: &Document, s: &Shape, force: bool) -> Vec<Gap> {
    let t = doc.layer(s.layer).main().tabs;
    if (!t.enabled && !force) || !t.auto_spacing || !s.tabs.is_empty() || t.size <= 0.0 {
        return Vec::new();
    }
    let subs = gpath::subpaths(&s.outline());
    // The closed outlines a shape can lie inside, as the job counts them: every closed outline
    // of its own layer, and those the other layers cut as lines.
    let around: Vec<BezPath> = if t.skip_inner { outlines_around(doc, s.layer) } else { Vec::new() };
    let mut out = Vec::new();
    for sub in &subs {
        if !gpath::is_closed(sub, 0.0) || (t.skip_inner && lies_inside_another(sub, &around)) {
            continue;
        }
        // The same count and tab length as the job (`darkburn_job::line`): the spacing gives the
        // count, held between Min tabs and Max tabs (Min wins when it is the larger), and the
        // tabs together never take more than half of the outline.
        let l = gpath::length(sub);
        if l <= 1e-9 {
            continue;
        }
        let n = ((l / t.spacing.max(0.1)).round() as u32).clamp(t.min_count, t.max_count.max(t.min_count)) as usize;
        if n == 0 {
            continue;
        }
        let opts = TabOptions {
            count: n,
            spacing: None,
            min_count: 0,
            max_count: usize::MAX,
            size: t.size.min(l / (2.0 * n as f64)),
            skip_shorter_than: None,
            offset_from_start: 0.0,
        };
        out.extend(tabs::place_tabs(sub, &opts).gaps);
    }
    out
}

/// The closed outlines that make a shape of `layer` an inner shape when it lies inside one: the
/// closed outlines of `layer` itself and those of the other layers that are cut as lines.
fn outlines_around(doc: &Document, layer: usize) -> Vec<BezPath> {
    let cuts_lines = |l: usize| {
        let l = doc.layer(l);
        l.output
            && l.settings.iter().any(|c| c.enabled && c.passes > 0 && matches!(c.mode, darkburn_core::Mode::Line | darkburn_core::Mode::FillLine))
    };
    doc.iter()
        .flat_map(Shape::leaves)
        .filter(|l| !l.is_image() && (l.layer == layer || cuts_lines(l.layer)))
        .flat_map(|l| gpath::subpaths(&l.outline()))
        .filter(|p| gpath::is_closed(p, 0.0))
        .collect()
}

/// Whether the closed outline `sub` lies inside a larger one of `around`.
fn lies_inside_another(sub: &BezPath, around: &[BezPath]) -> bool {
    let (Some(bb), Some(first)) = (gpath::bounding_box(sub), gpath::start_point(sub)) else { return false };
    let area = gpath::signed_area(sub).abs();
    let nodes: Vec<Point> = gpath::contours(sub).iter().flat_map(|c| c.nodes()).collect();
    let mid = nodes.get(nodes.len() / 2).copied().unwrap_or(first);
    around.iter().any(|q| {
        let Some(qb) = gpath::bounding_box(q) else { return false };
        let within = bb.x0 >= qb.x0 - 1e-9 && bb.y0 >= qb.y0 - 1e-9 && bb.x1 <= qb.x1 + 1e-9 && bb.y1 <= qb.y1 + 1e-9;
        within
            && gpath::signed_area(q).abs() > area
            && query::point_in_shape(q, first, darkburn_geom::FillRule::EvenOdd)
            && query::point_in_shape(q, mid, darkburn_geom::FillRule::EvenOdd)
    })
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
/// Used for the start and end markers of the Start Points tool. For a group: the outlines of
/// the shapes in it, each with its own.
pub fn marker_path(doc: &Document, id: ShapeId) -> Option<BezPath> {
    let leaves = cut_leaves(doc, id);
    (!leaves.is_empty()).then(|| leaves.iter().flat_map(|(_, l)| leaf_marker_path(l)).collect())
}

fn leaf_marker_path(s: &Shape) -> BezPath {
    let mut path = s.outline();
    for st in s.start_points() {
        let p = s.transform * Point::new(st[0], st[1]);
        // Only closed sub-paths have a start point to choose; the job uses the same rule.
        let mut subs = gpath::subpaths(&path);
        if let Some(k) = nearest_closed(&subs, p) {
            subs[k] = gpath::set_start_point(&subs[k], p, false);
            path = subs.iter().flat_map(|b| b.elements().iter().copied()).collect();
        }
    }
    match s.direction {
        CutDirection::Reversed => gpath::reverse(&path),
        CutDirection::Clockwise => gpath::set_direction(&path, gpath::Direction::Clockwise),
        CutDirection::CounterClockwise => gpath::set_direction(&path, gpath::Direction::CounterClockwise),
        CutDirection::Auto => path,
    }
}

/// The closed sub-path nearest to `p`.
fn nearest_closed(subs: &[BezPath], p: Point) -> Option<usize> {
    subs.iter()
        .enumerate()
        .filter(|(_, b)| darkburn_core::shape::path_is_closed(b))
        .filter_map(|(i, b)| query::nearest_point(b, p).map(|h| (i, h.distance)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The forced start points (local coordinates) plain shape `s` has after a click at `p`: the
/// point of its outline nearest to `p` on the nearest closed sub-path (`all`: on every closed
/// sub-path), and the start points it had on the sub-paths that were not touched. `s` carries
/// its document transform. None when it has no closed sub-path.
fn start_points_after(s: &Shape, p: Point, all: bool) -> Option<Vec<[f64; 2]>> {
    let subs = gpath::subpaths(&s.outline());
    let clicked = nearest_closed(&subs, p)?;
    let chosen = |k: usize| darkburn_core::shape::path_is_closed(&subs[k]) && (all || k == clicked);
    // The clicked sub-path first, then the others set with `all`.
    let mut new: Vec<Point> = Vec::new();
    for k in std::iter::once(clicked).chain((0..subs.len()).filter(|k| *k != clicked && chosen(*k))) {
        if let Some(h) = query::nearest_point(&subs[k], p) {
            new.push(h.point);
        }
    }
    let (t, inv) = (s.transform, s.transform.inverse());
    // Start points of the sub-paths that were not touched stay.
    let kept = s.start_points().map(|q| t * Point::new(q[0], q[1])).filter(|q| nearest_closed(&subs, *q).is_some_and(|k| !chosen(k)));
    Some(new.iter().copied().chain(kept).map(|q| inv * q).map(|q| [q.x, q.y]).collect())
}

/// Force where the cut of a closed shape starts: the point of its outline nearest to `x`, `y`.
/// A compound shape keeps one start point for each closed sub-path: the click sets the one of
/// the nearest sub-path (`all`: of every sub-path) and leaves the others alone. The points are
/// kept on the shape, so the cut planner starts there whatever it optimises; a path shape also
/// gets its nodes reordered, so the start node shows in Edit Nodes. In a group the click sets
/// the start of the shape nearest to it (`all`: of every shape in the group).
fn set_start(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let p = Point::new(num(a, "x", 0.0), num(a, "y", 0.0));
    let all = flag(a, "all", false);
    let done = e.edit("Set start point", |d, _| {
        let mut any = false;
        for id in &ids {
            let leaves = cut_leaves(d, *id);
            let grouped = d.get(*id).is_some_and(Shape::is_group);
            // The shapes the click is for: all of them, or the one with the nearest closed outline.
            let nearest = leaves
                .iter()
                .filter_map(|(n, l)| {
                    let subs = gpath::subpaths(&l.outline());
                    let k = nearest_closed(&subs, p)?;
                    query::nearest_point(&subs[k], p).map(|h| (*n, h.distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(n, _)| n);
            for (n, leaf) in leaves.iter().filter(|(n, _)| (all && grouped) || Some(*n) == nearest) {
                let Some(points) = start_points_after(leaf, p, all) else { continue };
                if !grouped && matches!(leaf.kind, ShapeKind::Path { .. }) {
                    let path = leaf.outline();
                    let q = gpath::set_start_point(&path, p, all);
                    if q != path {
                        any |= set_shape_path(d, *id, q);
                    }
                }
                let Some(s) = leaf_mut(d, *id, *n) else { continue };
                if s.start_points().collect::<Vec<_>>() != points {
                    s.set_start_points(&points);
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
                    s.direction = CutDirection::Auto;
                }
            } else if let Some(s) = d.get_mut(*id)
                && !s.is_group()
                && !s.is_image()
            {
                s.direction = if s.direction == CutDirection::Reversed { CutDirection::Auto } else { CutDirection::Reversed };
                any = true;
            }
        }
        any
    });
    ok(done, &ids)
}

/// Force the cut direction (`reversed`, `clockwise`, `counter_clockwise`) or clear the override
/// (`auto`, `none`) on the selected shapes. The geometry is not changed.
fn set_direction(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let dir = match word(a, "direction", "none") {
        "reversed" => CutDirection::Reversed,
        "clockwise" => CutDirection::Clockwise,
        "counter_clockwise" => CutDirection::CounterClockwise,
        "none" | "auto" | "as_drawn" => CutDirection::Auto,
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

/// Clear the forced start point of the selected shapes (in a group: of every shape in it).
fn clear_start(e: &mut Editor, a: &Value) -> R {
    let ids = targets(e, a)?;
    let done = e.edit("Clear start point", |d, _| {
        let mut any = false;
        for id in &ids {
            if !cut_leaves(d, *id).iter().any(|(_, l)| l.start_points().next().is_some()) {
                continue;
            }
            if let Some(s) = d.get_mut(*id) {
                any |= each_leaf_mut(s, darkburn_core::kurbo::Affine::IDENTITY, &mut |l, _| {
                    let had = l.start_points().next().is_some();
                    l.set_start_points(&[]);
                    had
                });
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
        let st = sh.start_point.expect("start recorded");
        assert!((st[0] - 5.0).abs() < 1e-6 || (st[0] - 10.0).abs() < 1e-6 || (st[0] + 5.0).abs() < 1e-6, "{st:?}");
        let mp = marker_path(&app.editor.doc, r).unwrap();
        let first = gpath::start_point(&mp).unwrap();
        assert!(first.distance(Point::new(10.0, 5.0)) < 1e-6, "marker starts at the click: {first:?}");
        run(&mut app, "shape.reverse_direction", json!({})).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, CutDirection::Reversed);
        run(&mut app, "shape.reverse_direction", json!({})).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, CutDirection::Auto);
        run(&mut app, "shape.set_direction", json!({ "direction": "clockwise" })).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().direction, CutDirection::Clockwise);
        assert!(run(&mut app, "shape.set_direction", json!({ "direction": "sideways" })).is_err());
        run(&mut app, "shape.set_direction", json!({ "direction": "none" })).unwrap();
        run(&mut app, "shape.clear_start_point", json!({})).unwrap();
        let sh = app.editor.doc.get(r).unwrap();
        assert!(sh.start_point.is_none() && sh.direction == CutDirection::Auto);
        // Undo restores the override.
        app.editor.undo();
        assert!(app.editor.doc.get(r).unwrap().start_point.is_some());
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
    fn start_points_work_on_compound_shapes_one_sub_path_at_a_time() {
        let mut app = app();
        let c = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 0 L20 0 L20 20 L0 20 Z M50 0 L70 0 L70 20 L50 20 Z" })).unwrap();
        let id = ShapeId(c["id"].as_u64().unwrap());
        select(&mut app, &[id]);
        let starts = |app: &crate::shell::DarkBurn| -> Vec<Point> {
            darkburn_geom::path::contours(&app.editor.doc.get(id).unwrap().outline()).iter().filter_map(|c| c.start()).collect()
        };
        assert_eq!(starts(&app), [Point::new(0.0, 0.0), Point::new(50.0, 0.0)]);
        run(&mut app, "shape.set_start_point", json!({ "x": 70.0, "y": 20.0 })).unwrap();
        assert_eq!(starts(&app), [Point::new(0.0, 0.0), Point::new(70.0, 20.0)], "only the nearest sub-path was turned");
        run(&mut app, "shape.set_start_point", json!({ "x": 20.0, "y": 20.0, "all": true })).unwrap();
        let s = starts(&app);
        assert_eq!(s[0], Point::new(20.0, 20.0), "with all, every sub-path starts at its nearest point: {s:?}");
        let m = marker_path(&app.editor.doc, id).unwrap();
        assert_eq!(darkburn_geom::path::contours(&m).len(), 2, "the markers see both sub-paths");
    }

    #[test]
    fn commands_with_their_own_points_or_shape_need_no_selection() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 100.0, 100.0, 4);
        app.editor.selection.clear();
        let m = run(&mut app, "measure.points", json!({ "a": [0, 0], "b": [30, 40] })).unwrap();
        assert!((m["distance"].as_f64().unwrap() - 50.0).abs() < 1e-9, "{m}");
        assert!(run(&mut app, "measure.arc", json!({ "a": [10, 0], "b": [0, 10], "c": [-10, 0] })).is_ok());
        run(&mut app, "shape.add_line", json!({ "points": [[200, 0], [220, 0]] })).unwrap();
        run(&mut app, "tabs.add", json!({ "shape": r.0, "x": 50, "y": 0 })).unwrap();
        assert_eq!(app.editor.doc.get(r).unwrap().tabs.len(), 1);
        run(&mut app, "shape.set_start_point", json!({ "shape": r.0, "x": 100, "y": 100 })).unwrap();
        assert!(app.editor.doc.get(r).unwrap().start_point.is_some());
    }

    /// Tabs and start points of a shape inside a group: no need to ungroup first.
    #[test]
    fn tabs_and_start_points_reach_shapes_inside_a_group() {
        let mut app = app();
        let a = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        let b = add_rect(&mut app, 50.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[a, b]);
        run(&mut app, "arrange.group", Value::Null).unwrap();
        let g = app.editor.selected()[0];
        // move the group, so the group's own placement matters
        app.editor.doc.get_mut(g).unwrap().transform =
            darkburn_core::kurbo::Affine::translate((100.0, 20.0)) * app.editor.doc.get(g).unwrap().transform;
        let leaves = |app: &crate::shell::DarkBurn| app.editor.doc.get(g).unwrap().leaves();
        let tabs_of = |app: &crate::shell::DarkBurn| -> Vec<Vec<Point>> {
            leaves(app).iter().map(|l| l.tabs.iter().map(|t| l.transform * Point::new(t[0], t[1])).collect()).collect()
        };
        // a tab on the bottom edge of the second square (now at 150..160, 20..30)
        let out = run(&mut app, "tabs.add", json!({ "shape": g.0, "x": 155.0, "y": 19.7 })).unwrap();
        assert_eq!(out["changed"], true, "{out}");
        let t = tabs_of(&app);
        assert!(t[0].is_empty(), "nothing on the first square");
        assert_eq!(t[1].len(), 1);
        assert!(t[1][0].distance(Point::new(155.0, 20.0)) < 1e-6, "{:?}", t[1][0]);
        // the tool sees it, and the automatic tabs of both squares
        assert_eq!(cut_outline(&app.editor.doc, g).map(|p| gpath::subpaths(&p).len()), Some(2));
        // a start point on the right edge of the first square (100..110, 20..30)
        let out = run(&mut app, "shape.set_start_point", json!({ "shape": g.0, "x": 110.3, "y": 25.0 })).unwrap();
        assert_eq!(out["changed"], true, "{out}");
        let l = leaves(&app);
        let start = l[0].start_point.map(|p| l[0].transform * Point::new(p[0], p[1])).unwrap();
        assert!(start.distance(Point::new(110.0, 25.0)) < 1e-6, "{start:?}");
        assert!(l[1].start_point.is_none());
        let m = marker_path(&app.editor.doc, g).unwrap();
        assert!(gpath::start_point(&gpath::subpaths(&m)[0]).unwrap().distance(Point::new(110.0, 25.0)) < 1e-6, "the marker shows it");
        // they are in the job: the cut of the first square starts there, the second has its gap
        let job = crate::job_build::build_job_simple(&crate::job_cmds::build_input(&app)).unwrap();
        assert!(job.warnings.is_empty(), "{:?}", job.warnings);
        // undo takes the start point back, then the tab
        assert!(app.editor.undo() && leaves(&app)[0].start_point.is_none());
        assert!(app.editor.redo());
        // take them off again
        assert_eq!(run(&mut app, "tabs.remove", json!({ "shape": g.0, "x": 155.0, "y": 20.0, "radius": 1.0 })).unwrap()["changed"], true);
        assert!(tabs_of(&app).iter().all(Vec::is_empty));
        run(&mut app, "tabs.add", json!({ "shape": g.0, "x": 105.0, "y": 30.0 })).unwrap();
        assert_eq!(tabs_of(&app)[0].len(), 1);
        assert_eq!(run(&mut app, "tabs.clear", json!({ "shape": g.0 })).unwrap()["changed"], true);
        assert!(tabs_of(&app).iter().all(Vec::is_empty));
        assert_eq!(run(&mut app, "shape.clear_start_point", json!({ "shape": g.0 })).unwrap()["changed"], true);
        assert!(leaves(&app).iter().all(|l| l.start_point.is_none()));
        // a picture still takes no tabs
        assert!(run(&mut app, "tabs.add", json!({ "shape": 999_999, "x": 0, "y": 0 })).is_err());
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
