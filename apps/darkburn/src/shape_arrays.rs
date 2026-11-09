//! Arrays (grid, circular, copy along a path, and the virtual arrays that stay editable),
//! nesting and docking. Part of the shape-editing commands; see `shape_ops.rs`.

use darkburn_core::arrange;
use darkburn_core::kurbo::{Affine, BezPath, Rect, Vec2};
use darkburn_core::varray::{self, AlongDef, ArrayDef, ArrayKind, CircularDef, GridDef, GridShift};
use darkburn_core::{Document, Editor, Shape, ShapeId, ShapeKind};
use darkburn_geom::array as garray;
use darkburn_geom::nest::{self, NestOptions};
use serde_json::{Value, json};

use crate::shape_ops::{Env, R, flag, in_draw_order, num, ok, word};
use crate::shell::DarkBurn;

pub fn works(id: &str) -> bool {
    matches!(
        id,
        "arrange.grid_array"
            | "arrange.circular_array"
            | "arrange.copy_along_path"
            | "arrange.array_edit"
            | "arrange.array_add"
            | "arrange.array_remove"
            | "arrange.array_exit"
            | "arrange.array_flatten"
            | "arrange.array_duplicate"
            | "arrange.nest"
            | "arrange.dock_left"
            | "arrange.dock_right"
            | "arrange.dock_up"
            | "arrange.dock_down"
            | "arrange.alignment_helper"
            | "arrange.rotate_by"
    )
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "arrange.grid_array" => &["ARR-30", "ARRAY-01", "ARRAY-02", "ARRAY-03", "ARRAY-04", "ARRAY-06", "TB-10"],
        "arrange.circular_array" => &["ARR-31", "ARRAY-07", "TB-10"],
        "arrange.copy_along_path" => &["ARR-32", "ARRAY-08"],
        "arrange.array_edit"
        | "arrange.array_add"
        | "arrange.array_remove"
        | "arrange.array_exit"
        | "arrange.array_flatten"
        | "arrange.array_duplicate" => &["ARR-41", "ARRAY-05"],
        "arrange.nest" => &["ARR-25", "ARRAY-09"],
        "arrange.dock_left" | "arrange.dock_right" | "arrange.dock_up" | "arrange.dock_down" => &["ARR-26", "TB-24", "ARRAY-13"],
        "arrange.alignment_helper" => &["ARRAY-12"],
        "arrange.rotate_by" => &["CANVAS-19", "ARR-07", "ARR-08"],
        _ => &[],
    }
}

pub fn args_schema(id: &str) -> Option<&'static str> {
    Some(match id {
        "arrange.grid_array" => {
            r#"{"type":"object","properties":{"rows":{"type":"integer"},"cols":{"type":"integer"},"fit_width":{"type":"number","description":"fill this width with as many columns as fit"},"fit_height":{"type":"number"},"centre_to_centre":{"type":"boolean"},"dx":{"type":"number","description":"gap or step in X, mm"},"dy":{"type":"number"},"shift":{"enum":["none","alt_rows","alt_cols","hex"]},"shift_amount":{"type":"number"},"mirror_alt_rows":{"type":"boolean"},"mirror_alt_cols":{"type":"boolean"},"reverse_x":{"type":"boolean"},"reverse_y":{"type":"boolean"},"virtual":{"type":"boolean","description":"keep the array editable"}}}"#
        }
        "arrange.circular_array" => {
            r#"{"type":"object","properties":{"cx":{"type":"number"},"cy":{"type":"number"},"count":{"type":"integer"},"start_angle":{"type":"number"},"sweep":{"type":"number"},"rotate_items":{"type":"boolean"},"clockwise":{"type":"boolean"},"virtual":{"type":"boolean"}}}"#
        }
        "arrange.copy_along_path" => {
            r#"{"type":"object","properties":{"path_shape":{"type":"integer","description":"id of the path to follow; the last selected shape when missing"},"count":{"type":"integer"},"spacing":{"type":"number"},"rotate_items":{"type":"boolean"},"start_at_nearest":{"type":"boolean"},"virtual":{"type":"boolean"}}}"#
        }
        "arrange.array_edit" => {
            r#"{"type":"object","properties":{"id":{"type":"integer","description":"the virtual array; the selected one when missing"},"kind":{"enum":["grid","circular","along_path"]}},"description":"plus any setting of that array kind"}"#
        }
        "arrange.array_remove" => {
            r#"{"type":"object","properties":{"index":{"type":"integer","description":"which original item; the last when missing"}}}"#
        }
        "arrange.array_duplicate" => r#"{"type":"object","properties":{"dx":{"type":"number"},"dy":{"type":"number"}}}"#,
        "arrange.nest" => {
            r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"width":{"type":"number"},"height":{"type":"number"},"spacing":{"type":"number"},"margin":{"type":"number"},"rotation_step":{"type":"number","description":"degrees; 0 for none"},"allow_mirror":{"type":"boolean"}},"description":"the sheet is the work area when x, y, width, height are missing"}"#
        }
        "arrange.dock_left" | "arrange.dock_right" | "arrange.dock_up" | "arrange.dock_down" => {
            r#"{"type":"object","properties":{"padding":{"type":"number"},"as_group":{"type":"boolean"},"lock_inner":{"type":"boolean"}}}"#
        }
        _ => return None,
    })
}

pub fn can_run(app: &DarkBurn, id: &str) -> bool {
    let has_array = app.editor.selected().iter().any(|i| app.editor.doc.get(*i).is_some_and(|s| s.array.is_some()));
    match id {
        "arrange.array_add" => has_array && app.editor.selection.len() >= 2,
        _ => has_array,
    }
}

/// Extra arguments the interface passes when it runs `id` without a dialog.
pub fn run_args(_app: &DarkBurn, _id: &str) -> Value {
    Value::Null
}

// ---------------------------------------------------------------- settings from arguments

/// Grid settings: `a` over `base`.
pub fn grid_from(a: &Value, base: &GridDef) -> GridDef {
    let amount = num(
        a,
        "shift_amount",
        match base.shift {
            GridShift::AltRows(v) | GridShift::AltCols(v) => v,
            _ => 0.0,
        },
    );
    let shift = match a.get("shift").and_then(Value::as_str) {
        Some("none") => GridShift::None,
        Some("alt_rows") => GridShift::AltRows(amount),
        Some("alt_cols") => GridShift::AltCols(amount),
        Some("hex") => GridShift::Hex,
        _ => match base.shift {
            GridShift::AltRows(_) => GridShift::AltRows(amount),
            GridShift::AltCols(_) => GridShift::AltCols(amount),
            other => other,
        },
    };
    GridDef {
        rows: num(a, "rows", base.rows as f64).max(1.0) as usize,
        cols: num(a, "cols", base.cols as f64).max(1.0) as usize,
        centre_to_centre: flag(a, "centre_to_centre", base.centre_to_centre),
        dx: num(a, "dx", base.dx),
        dy: num(a, "dy", base.dy),
        shift,
        mirror_alt_rows: flag(a, "mirror_alt_rows", base.mirror_alt_rows),
        mirror_alt_cols: flag(a, "mirror_alt_cols", base.mirror_alt_cols),
        reverse_x: flag(a, "reverse_x", base.reverse_x),
        reverse_y: flag(a, "reverse_y", base.reverse_y),
    }
}

pub fn circ_from(a: &Value, base: &CircularDef) -> CircularDef {
    CircularDef {
        cx: num(a, "cx", base.cx),
        cy: num(a, "cy", base.cy),
        count: num(a, "count", base.count as f64).clamp(1.0, 10_000.0) as usize,
        start_angle: num(a, "start_angle", base.start_angle),
        sweep: num(a, "sweep", base.sweep),
        rotate_items: flag(a, "rotate_items", base.rotate_items),
        clockwise: flag(a, "clockwise", base.clockwise),
    }
}

pub fn along_from(a: &Value, base: &AlongDef) -> AlongDef {
    AlongDef {
        path: base.path.clone(),
        count: num(a, "count", base.count as f64).clamp(1.0, 10_000.0) as usize,
        spacing: num(a, "spacing", base.spacing).max(0.0),
        rotate_items: flag(a, "rotate_items", base.rotate_items),
        start_at_nearest: flag(a, "start_at_nearest", base.start_at_nearest),
    }
}

/// The copies a definition would make, as whole shapes (for previews).
pub fn array_preview(def: &ArrayDef) -> Vec<BezPath> {
    def.copies().iter().map(Shape::outline).collect()
}

// ---------------------------------------------------------------- make arrays

fn items_of(e: &Editor, ids: &[ShapeId]) -> Vec<Shape> {
    ids.iter().filter_map(|i| e.doc.get(*i)).cloned().collect()
}

/// Put the array `def` into the document in place of `ids` (virtual), or as real copies.
fn place_array(e: &mut Editor, ids: &[ShapeId], def: ArrayDef, virtual_array: bool, label: &str) -> R {
    let transforms = def.transforms();
    if transforms.is_empty() {
        return Err("the array is empty".into());
    }
    if transforms.len() > 100_000 {
        return Err("that is too many copies".into());
    }
    let layer = def.source.first().map(|s| s.layer).unwrap_or(0);
    let mut out = Vec::new();
    e.edit(label, |d, sel| {
        if virtual_array {
            let at = ids.iter().filter_map(|i| d.index_of(*i)).min().unwrap_or(d.len());
            d.remove(ids);
            let mut g = Shape::new(ShapeKind::Group { children: Vec::new() }, Affine::IDENTITY, layer);
            g.array = Some(Box::new(def));
            varray::regenerate(&mut g, || ShapeId(0));
            let id = d.insert(at.min(d.len()), g);
            out = vec![id];
        } else {
            // Copy 0 may itself be moved (a circular array with a start angle).
            if transforms[0] != Affine::IDENTITY {
                arrange::transform(d, ids, transforms[0]);
            }
            out = ids.to_vec();
            for (i, t) in transforms.iter().enumerate().skip(1) {
                for s in &def.source {
                    let mut c = s.clone();
                    c.transform = *t * c.transform;
                    c.array = None;
                    varray::advance_text(&mut c, i as i64);
                    out.push(d.add(c));
                }
            }
        }
        sel.set(out.iter().copied());
        true
    });
    ok(true, &out)
}

fn selected_items(e: &Editor) -> Result<Vec<ShapeId>, String> {
    let ids = in_draw_order(&e.doc, &e.selected());
    if ids.is_empty() { Err("nothing is selected".into()) } else { Ok(ids) }
}

fn grid_array(e: &mut Editor, a: &Value) -> R {
    let ids = selected_items(e)?;
    let source = items_of(e, &ids);
    let mut g = grid_from(a, &GridDef::default());
    if let (Some(_), true) = (a.get("fit_width").or(a.get("fit_height")), true) {
        let bb = varray::bbox_of(&source).ok_or("nothing to array")?;
        let (w, h) = (num(a, "fit_width", f64::INFINITY), num(a, "fit_height", f64::INFINITY));
        let target = Rect::new(bb.x0, bb.y0, bb.x0 + if w.is_finite() { w } else { bb.width() }, bb.y0 + if h.is_finite() { h } else { bb.height() });
        let (rows, cols) = garray::grid_fit(bb, target, g.options().spacing);
        if h.is_finite() {
            g.rows = rows.max(1);
        }
        if w.is_finite() {
            g.cols = cols.max(1);
        }
    }
    place_array(e, &ids, ArrayDef { source, kind: ArrayKind::Grid(g) }, flag(a, "virtual", false), "Grid array")
}

fn circular_array(e: &mut Editor, a: &Value, env: &Env) -> R {
    let ids = selected_items(e)?;
    let source = items_of(e, &ids);
    let bb = varray::bbox_of(&source).ok_or("nothing to array")?;
    let base = CircularDef { cx: env.page.center().x, cy: env.page.center().y, ..CircularDef::default() };
    let _ = bb;
    let c = circ_from(a, &base);
    place_array(e, &ids, ArrayDef { source, kind: ArrayKind::Circular(c) }, flag(a, "virtual", false), "Circular array")
}

fn copy_along_path(e: &mut Editor, a: &Value) -> R {
    let mut ids = selected_items(e)?;
    let path_id = match a.get("path_shape").and_then(Value::as_u64) {
        Some(p) => ShapeId(p),
        None => *ids.last().ok_or("nothing is selected")?,
    };
    ids.retain(|i| *i != path_id);
    if ids.is_empty() {
        return Err("select the shapes to copy and the path to follow".into());
    }
    let path = crate::shape_ops::shape_path(&e.doc, path_id).ok_or("the path shape cannot be used")?;
    let source = items_of(e, &ids);
    let mut base = AlongDef { path, ..AlongDef::default() };
    base = along_from(a, &base);
    place_array(e, &ids, ArrayDef { source, kind: ArrayKind::AlongPath(base) }, flag(a, "virtual", false), "Copy along path")
}

// ---------------------------------------------------------------- virtual arrays

fn array_target(e: &Editor, a: &Value) -> Result<ShapeId, String> {
    if let Some(id) = a.get("id").and_then(Value::as_u64) {
        return Ok(ShapeId(id));
    }
    e.selected().into_iter().find(|i| e.doc.get(*i).is_some_and(|s| s.array.is_some())).ok_or_else(|| "select a virtual array".into())
}

fn regenerate_at(d: &mut Document, id: ShapeId) {
    let Some(i) = d.index_of(id) else { return };
    let mut s = Shape::clone(&d.shapes[i]);
    varray::regenerate(&mut s, || d.alloc_id());
    d.shapes[i] = std::sync::Arc::new(s);
}

fn array_edit(e: &mut Editor, a: &Value, env: &Env) -> R {
    let id = array_target(e, a)?;
    let def = e.doc.get(id).and_then(|s| s.array.as_deref().cloned()).ok_or("that shape is not a virtual array")?;
    let kind = match (word(a, "kind", ""), &def.kind) {
        ("grid", ArrayKind::Grid(_)) | ("", ArrayKind::Grid(_)) => ArrayKind::Grid(grid_from(a, &grid_of(&def.kind))),
        ("grid", _) => ArrayKind::Grid(grid_from(a, &GridDef::default())),
        ("circular", ArrayKind::Circular(_)) | ("", ArrayKind::Circular(_)) => ArrayKind::Circular(circ_from(a, &circ_of(&def.kind))),
        ("circular", _) => ArrayKind::Circular(circ_from(a, &CircularDef { cx: env.page.center().x, cy: env.page.center().y, ..Default::default() })),
        ("along_path", ArrayKind::AlongPath(p)) | ("", ArrayKind::AlongPath(p)) => ArrayKind::AlongPath(along_from(a, p)),
        (other, _) => return Err(format!("cannot change the array to kind {other:?}")),
    };
    let done = e.edit("Edit array", |d, sel| {
        if let Some(s) = d.get_mut(id)
            && let Some(def) = s.array.as_mut()
        {
            def.kind = kind;
        }
        regenerate_at(d, id);
        sel.set([id]);
        true
    });
    ok(done, &[id])
}

fn grid_of(k: &ArrayKind) -> GridDef {
    if let ArrayKind::Grid(g) = k { *g } else { GridDef::default() }
}

fn circ_of(k: &ArrayKind) -> CircularDef {
    if let ArrayKind::Circular(c) = k { *c } else { CircularDef::default() }
}

fn array_add(e: &mut Editor, a: &Value) -> R {
    let id = array_target(e, a)?;
    let others: Vec<ShapeId> = in_draw_order(&e.doc, &e.selected()).into_iter().filter(|i| *i != id).collect();
    if others.is_empty() {
        return Err("select the shapes to add together with the array".into());
    }
    let done = e.edit("Add to array", |d, sel| {
        let Some(group) = d.get(id).cloned() else { return false };
        let inv = group.transform.inverse();
        let added: Vec<Shape> = others
            .iter()
            .filter_map(|i| d.get(*i))
            .map(|s| {
                let mut c = s.clone();
                c.transform = inv * c.transform;
                c
            })
            .collect();
        d.remove(&others);
        if let Some(s) = d.get_mut(id)
            && let Some(def) = s.array.as_mut()
        {
            def.source.extend(added);
        }
        regenerate_at(d, id);
        sel.set([id]);
        true
    });
    ok(done, &[id])
}

fn array_remove(e: &mut Editor, a: &Value) -> R {
    let id = array_target(e, a)?;
    let n = e.doc.get(id).and_then(|s| s.array.as_ref().map(|d| d.source.len())).unwrap_or(0);
    if n == 0 {
        return Err("that shape is not a virtual array".into());
    }
    let index = num(a, "index", (n - 1) as f64) as usize;
    if index >= n {
        return Err(format!("the array has {n} original items"));
    }
    let mut freed = Vec::new();
    let done = e.edit("Remove from array", |d, sel| {
        let Some(group) = d.get(id).cloned() else { return false };
        let mut item = group.array.as_ref().map(|a| a.source[index].clone()).unwrap_or_default();
        item.transform = group.transform * item.transform;
        if n == 1 {
            // The last original leaves: the array is gone.
            d.remove(&[id]);
        } else {
            if let Some(s) = d.get_mut(id)
                && let Some(def) = s.array.as_mut()
            {
                def.source.remove(index);
            }
            regenerate_at(d, id);
        }
        freed.push(d.add(item));
        sel.set(freed.iter().copied());
        true
    });
    ok(done, &freed)
}

fn array_flatten(e: &mut Editor, _a: &Value) -> R {
    let ids: Vec<ShapeId> = e.selected().into_iter().filter(|i| e.doc.get(*i).is_some_and(|s| s.array.is_some())).collect();
    if ids.is_empty() {
        return Err("select a virtual array".into());
    }
    let mut out = Vec::new();
    let done = e.edit("Flatten array", |d, sel| {
        for id in &ids {
            if let Some(s) = d.get_mut(*id) {
                s.array = None;
            }
            out.extend(arrange::ungroup(d, &[*id]));
        }
        sel.set(out.iter().copied());
        true
    });
    ok(done, &out)
}

fn array_duplicate(e: &mut Editor, a: &Value) -> R {
    let id = array_target(e, a)?;
    let (dx, dy) = (num(a, "dx", 0.0), num(a, "dy", 0.0));
    let mut out = Vec::new();
    let done = e.edit("Duplicate array", |d, sel| {
        let Some(mut s) = d.get(id).cloned() else { return false };
        s.transform = Affine::translate(Vec2::new(dx, dy)) * s.transform;
        out = vec![d.add(s)];
        sel.set(out.iter().copied());
        true
    });
    ok(done, &out)
}

// ---------------------------------------------------------------- nesting

fn nest_cmd(e: &mut Editor, a: &Value, env: &Env) -> R {
    let ids = selected_items(e)?;
    let sheet = match (a.get("width"), a.get("height")) {
        (Some(_), Some(_)) => {
            let (x, y) = (num(a, "x", env.page.x0), num(a, "y", env.page.y0));
            Rect::new(x, y, x + num(a, "width", env.page.width()), y + num(a, "height", env.page.height()))
        }
        _ => env.page,
    };
    let opts = NestOptions {
        spacing: num(a, "spacing", 2.0).max(0.0),
        margin: num(a, "margin", 0.0).max(0.0),
        rotations: nest::rotation_steps(num(a, "rotation_step", 90.0)),
        allow_mirror: flag(a, "allow_mirror", false),
    };
    let shapes: Vec<BezPath> = ids
        .iter()
        .map(|i| {
            let mut all = BezPath::new();
            for p in crate::shape_ops::parts_of(&e.doc, *i) {
                all.extend(p.path);
            }
            all
        })
        .collect();
    let result = nest::nest(&shapes, sheet, &opts);
    let placed = result.placements.iter().flatten().count();
    let done = e.edit("Nest", |d, _| {
        let mut any = false;
        for (id, p) in ids.iter().zip(&result.placements) {
            if let Some(t) = p {
                any |= arrange::transform(d, &[*id], *t);
            }
        }
        any
    });
    Ok(json!({ "changed": done, "placed": placed, "unplaced": result.unplaced, "utilization": result.utilization }))
}

// ---------------------------------------------------------------- docking

use crate::dock_geom::{self, Body};

#[derive(Clone, Copy)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    fn vector(self) -> Vec2 {
        match self {
            Dir::Left => Vec2::new(-1.0, 0.0),
            Dir::Right => Vec2::new(1.0, 0.0),
            Dir::Up => Vec2::new(0.0, 1.0),
            Dir::Down => Vec2::new(0.0, -1.0),
        }
    }

    fn to_edge(self, m: Rect, page: Rect) -> f64 {
        match self {
            Dir::Left => m.x0 - page.x0,
            Dir::Right => page.x1 - m.x1,
            Dir::Up => page.y1 - m.y1,
            Dir::Down => m.y0 - page.y0,
        }
    }

    /// Sort key: the shapes nearest the edge they travel towards go first.
    fn order(self, r: Rect) -> f64 {
        match self {
            Dir::Left => r.x0,
            Dir::Right => -r.x1,
            Dir::Up => -r.y1,
            Dir::Down => r.y0,
        }
    }
}

/// How far `m` moves in `dir` among `obstacles` (padding from shapes, none from the page edge).
/// Shapes stop where their outlines touch, not their bounding boxes.
fn travel(dir: Dir, m: &Body, obstacles: &[Body], page: Rect, padding: f64) -> f64 {
    let hit = obstacles.iter().filter_map(|o| dock_geom::gap(dir.vector(), m, o)).fold(f64::INFINITY, f64::min);
    let edge = dir.to_edge(m.bbox, page);
    let d = if hit.is_finite() && hit - padding <= edge { hit - padding } else { edge };
    d.max(0.0)
}

fn body_of(s: &Shape) -> Body {
    Body::from_path(&s.outline(), s.bbox())
}

fn dock(e: &mut Editor, a: &Value, env: &Env, dir: Dir) -> R {
    let ids = selected_items(e)?;
    let padding = num(a, "padding", env.padding).max(0.0);
    let as_group = flag(a, "as_group", env.dock_as_group);
    let lock_inner = flag(a, "lock_inner", env.dock_lock_inner);
    let bodies: Vec<(ShapeId, Body)> = ids.iter().filter_map(|i| e.doc.get(*i).map(|s| (*i, body_of(s)))).collect();
    let rects: Vec<(ShapeId, Rect)> = bodies.iter().map(|(i, b)| (*i, b.bbox)).collect();
    let mut obstacles: Vec<Body> = e.doc.iter().filter(|s| !ids.contains(&s.id)).map(body_of).collect();
    let mut moves: Vec<(Vec<ShapeId>, Vec2)> = Vec::new();
    if as_group {
        let all: Vec<ShapeId> = rects.iter().map(|r| r.0).collect();
        let bb = rects.iter().map(|r| r.1).reduce(|a, b| a.union(b)).ok_or("nothing to dock")?;
        let group = Body { bbox: bb, rings: bodies.iter().flat_map(|b| b.1.rings.iter().cloned()).collect() };
        moves.push((all, dir.vector() * travel(dir, &group, &obstacles, env.page, padding)));
    } else {
        let contains = |outer: Rect, inner: Rect| outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1;
        // Shapes inside another selected shape follow it when inner objects are locked.
        let mut leaders: Vec<(ShapeId, Rect, Vec<ShapeId>)> = Vec::new();
        for (i, (id, r)) in rects.iter().enumerate() {
            let inside = lock_inner && rects.iter().enumerate().any(|(j, (_, o))| i != j && contains(*o, *r) && (*o != *r || j < i));
            if !inside {
                leaders.push((*id, *r, vec![*id]));
            }
        }
        if lock_inner {
            for (id, r) in &rects {
                if leaders.iter().any(|l| l.0 == *id) {
                    continue;
                }
                if let Some(l) = leaders
                    .iter_mut()
                    .filter(|l| contains(l.1, *r))
                    .min_by(|a, b| (a.1.width() * a.1.height()).total_cmp(&(b.1.width() * b.1.height())))
                {
                    l.2.push(*id);
                }
            }
        }
        leaders.sort_by(|a, b| dir.order(a.1).total_cmp(&dir.order(b.1)));
        for (lid, _, members) in leaders {
            let Some(body) = bodies.iter().find(|b| b.0 == lid).map(|b| b.1.clone()) else { continue };
            let dist = travel(dir, &body, &obstacles, env.page, padding);
            let v = dir.vector() * dist;
            obstacles.push(body.translated(v));
            moves.push((members, v));
        }
    }
    let moved = moves.iter().any(|m| m.1.hypot() > 1e-9);
    let done = e.edit("Dock", |d, _| {
        let mut any = false;
        for (members, v) in &moves {
            if v.hypot() > 1e-9 {
                any |= arrange::translate(d, members, *v);
            }
        }
        any
    });
    Ok(json!({ "changed": done && moved }))
}

/// Rotate the selection by `degrees` (counter-clockwise) about its centre.
fn rotate_by(e: &mut Editor, a: &Value) -> R {
    let ids = selected_items(e)?;
    let degrees = num(a, "degrees", 0.0);
    let done = e.edit("Rotate", |d, _| arrange::rotate(d, &ids, degrees));
    ok(done, &ids)
}

pub fn exec(e: &mut Editor, env: &Env, id: &str, a: &Value) -> Option<R> {
    Some(match id {
        "arrange.grid_array" => grid_array(e, a),
        "arrange.circular_array" => circular_array(e, a, env),
        "arrange.copy_along_path" => copy_along_path(e, a),
        "arrange.array_edit" => array_edit(e, a, env),
        "arrange.array_add" => array_add(e, a),
        "arrange.array_remove" => array_remove(e, a),
        "arrange.array_exit" => ok(false, &[]),
        "arrange.array_flatten" => array_flatten(e, a),
        "arrange.array_duplicate" => array_duplicate(e, a),
        "arrange.nest" => nest_cmd(e, a, env),
        "arrange.rotate_by" => rotate_by(e, a),
        "arrange.dock_left" => dock(e, a, env, Dir::Left),
        "arrange.dock_right" => dock(e, a, env, Dir::Right),
        "arrange.dock_up" => dock(e, a, env, Dir::Up),
        "arrange.dock_down" => dock(e, a, env, Dir::Down),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_ops::tests::{add_rect, app, run, select};
    use darkburn_core::project;

    #[test]
    fn real_grid_and_undo() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 2);
        select(&mut app, &[r]);
        run(&mut app, "arrange.grid_array", json!({ "rows": 2, "cols": 3, "dx": 5, "dy": 5 })).unwrap();
        assert_eq!(app.editor.doc.len(), 6);
        let bb = app.editor.doc.bbox().unwrap();
        assert!((bb.width() - 40.0).abs() < 1e-6 && (bb.height() - 25.0).abs() < 1e-6);
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 1);
    }

    #[test]
    fn grid_options_hex_mirror_fit() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 0);
        select(&mut app, &[r]);
        run(
            &mut app,
            "arrange.grid_array",
            json!({ "fit_width": 45, "fit_height": 25, "centre_to_centre": true, "dx": 15, "dy": 15, "shift": "alt_rows", "shift_amount": 5 }),
        )
        .unwrap();
        // 45 wide fits 3 columns (0, 15, 30 -> right edge 40); 25 high fits 2 rows.
        assert_eq!(app.editor.doc.len(), 6, "{}", app.editor.doc.len());
        let mut app2 = crate::shape_ops::tests::app();
        let r = add_rect(&mut app2, 0.0, 0.0, 10.0, 10.0, 0);
        select(&mut app2, &[r]);
        run(&mut app2, "arrange.grid_array", json!({ "rows": 2, "cols": 2, "shift": "hex", "mirror_alt_rows": true, "reverse_x": true })).unwrap();
        assert_eq!(app2.editor.doc.len(), 4);
    }

    #[test]
    fn circular_and_along_path() {
        let mut app = app();
        let r = add_rect(&mut app, 20.0, -2.0, 4.0, 4.0, 0);
        select(&mut app, &[r]);
        run(&mut app, "arrange.circular_array", json!({ "cx": 0, "cy": 0, "count": 4 })).unwrap();
        assert_eq!(app.editor.doc.len(), 4);
        let bb = app.editor.doc.bbox().unwrap();
        assert!((bb.x0 + 24.0).abs() < 0.01 && (bb.x1 - 24.0).abs() < 0.01, "{bb:?}");
        let mut app = crate::shape_ops::tests::app();
        let r = add_rect(&mut app, 0.0, 0.0, 2.0, 2.0, 0);
        let p = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 50 L100 50" })).unwrap();
        select(&mut app, &[r, ShapeId(p["id"].as_u64().unwrap())]);
        run(&mut app, "arrange.copy_along_path", json!({ "count": 5, "rotate_items": false })).unwrap();
        assert_eq!(app.editor.doc.len(), 6, "five copies and the path");
    }

    #[test]
    fn virtual_array_edit_add_remove_flatten_and_save() {
        let mut app = app();
        let r = add_rect(&mut app, 0.0, 0.0, 10.0, 10.0, 1);
        select(&mut app, &[r]);
        let out = run(&mut app, "arrange.grid_array", json!({ "rows": 1, "cols": 3, "dx": 5, "dy": 0, "virtual": true })).unwrap();
        assert_eq!(app.editor.doc.len(), 1);
        let gid = ShapeId(out["ids"][0].as_u64().unwrap());
        let g = app.editor.doc.get(gid).unwrap();
        assert!(g.array.is_some() && matches!(&g.kind, ShapeKind::Group { children } if children.len() == 3));
        // Edit the settings: the array stays one editable object.
        run(&mut app, "arrange.array_edit", json!({ "cols": 5 })).unwrap();
        assert!(matches!(&app.editor.doc.get(gid).unwrap().kind, ShapeKind::Group { children } if children.len() == 5));
        // A layer change survives editing the array.
        run(&mut app, "shape.set_layer", json!({ "layer": 7 })).unwrap();
        run(&mut app, "arrange.array_edit", json!({ "cols": 4 })).unwrap();
        assert!(app.editor.doc.get(gid).unwrap().leaves().iter().all(|l| l.layer == 7));
        run(&mut app, "arrange.array_edit", json!({ "cols": 5 })).unwrap();
        // Add another original.
        let extra = add_rect(&mut app, 0.0, 30.0, 4.0, 4.0, 1);
        select(&mut app, &[gid, extra]);
        run(&mut app, "arrange.array_add", Value::Null).unwrap();
        assert!(matches!(&app.editor.doc.get(gid).unwrap().kind, ShapeKind::Group { children } if children.len() == 10));
        // Project save and load keeps the array.
        let bytes = project::save(&app.editor.doc, "test", None).unwrap();
        let loaded = project::load(&bytes).unwrap();
        let lg = loaded.doc.iter().next().unwrap();
        assert_eq!(lg.array.as_ref().unwrap().source.len(), 2);
        // Remove one original, then undo and redo.
        select(&mut app, &[gid]);
        run(&mut app, "arrange.array_remove", json!({ "index": 1 })).unwrap();
        assert!(matches!(&app.editor.doc.get(gid).unwrap().kind, ShapeKind::Group { children } if children.len() == 5));
        assert_eq!(app.editor.doc.len(), 2, "the removed original is a free shape again");
        run(&mut app, "edit.undo", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 1);
        run(&mut app, "edit.redo", Value::Null).unwrap();
        select(&mut app, &[gid]);
        run(&mut app, "arrange.array_duplicate", json!({ "dx": 0, "dy": 40 })).unwrap();
        assert_eq!(app.editor.doc.len(), 3);
        run(&mut app, "arrange.array_flatten", Value::Null).unwrap();
        assert_eq!(app.editor.doc.len(), 7, "five loose copies plus the free original and the other array");
        assert!(app.editor.doc.iter().all(|s| s.array.is_none() || s.is_group()));
        assert!(run(&mut app, "arrange.array_edit", json!({})).is_err() || app.editor.selected().len() == 5);
    }

    #[test]
    fn nest_places_inside_the_sheet() {
        let mut app = app();
        let ids: Vec<ShapeId> = (0..4).map(|i| add_rect(&mut app, 300.0 + i as f64 * 5.0, 300.0, 40.0, 20.0, 0)).collect();
        select(&mut app, &ids);
        let out = run(&mut app, "arrange.nest", json!({ "x": 0, "y": 0, "width": 100, "height": 100, "spacing": 2 })).unwrap();
        assert_eq!(out["unplaced"], 0, "{out}");
        for s in app.editor.doc.iter() {
            let b = s.bbox();
            assert!(b.x0 >= -1e-6 && b.y0 >= -1e-6 && b.x1 <= 100.0 + 1e-6 && b.y1 <= 100.0 + 1e-6, "{b:?}");
        }
    }

    #[test]
    fn docking_follows_outlines_not_boxes() {
        let mut app = app();
        // A triangle pointing left: its box spans x 100..140 but its point is at the left.
        let tri = run(&mut app, "shape.add_path", json!({ "svg_path": "M140 0 L140 100 L100 50 Z" })).unwrap();
        let _ = tri;
        // A small square near the top of the triangle's box, outside the outline there.
        let s = add_rect(&mut app, 20.0, 90.0, 10.0, 10.0, 0);
        select(&mut app, &[s]);
        run(&mut app, "arrange.dock_right", json!({})).unwrap();
        let x1 = app.editor.doc.get(s).unwrap().bbox().x1;
        // By boxes it would stop at x = 100; by outline it slides into the triangle's notch
        // and stops where the slanted edge crosses y = 90 (x = 132).
        assert!(x1 > 110.0 && x1 < 140.0, "slid past the box edge at 100: stopped at {x1}");
    }

    #[test]
    fn docking_stops_at_neighbours_and_page_edge() {
        let mut app = app();
        let wall = add_rect(&mut app, 0.0, 0.0, 10.0, 100.0, 0);
        let m = add_rect(&mut app, 50.0, 20.0, 10.0, 10.0, 0);
        let _ = wall;
        select(&mut app, &[m]);
        run(&mut app, "arrange.dock_left", json!({ "padding": 2 })).unwrap();
        assert!((app.editor.doc.get(m).unwrap().bbox().x0 - 12.0).abs() < 1e-6);
        run(&mut app, "arrange.dock_down", Value::Null).unwrap();
        assert!(app.editor.doc.get(m).unwrap().bbox().y0.abs() < 1e-6, "down to the page edge");
        run(&mut app, "arrange.dock_right", Value::Null).unwrap();
        assert!((app.editor.doc.get(m).unwrap().bbox().x1 - 400.0).abs() < 1e-6);
        // Group mode moves the block by the smallest distance.
        let a = add_rect(&mut app, 100.0, 200.0, 10.0, 10.0, 0);
        let b = add_rect(&mut app, 150.0, 200.0, 10.0, 10.0, 0);
        select(&mut app, &[a, b]);
        run(&mut app, "arrange.dock_up", json!({ "as_group": true })).unwrap();
        assert!((app.editor.doc.get(a).unwrap().bbox().y1 - 400.0).abs() < 1e-6);
        assert!((app.editor.doc.get(b).unwrap().bbox().y1 - 400.0).abs() < 1e-6);
    }
}
