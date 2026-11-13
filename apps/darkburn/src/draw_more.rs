//! More drawing and selection commands: select shapes like the selected ones, a star, a
//! circle or an arc through three points, and a gap left in closed outlines. They are document
//! edits and run without a window.

use darkburn_core::kurbo::{Arc, BezPath, PathEl, Point, Rect, Shape as _, Vec2};
use darkburn_core::{CutDirection, Shape, ShapeId, ShapeKind};
use darkburn_geom::path::{self as gpath, Contour, Direction};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;

const fn cmd(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    cmd(
        "edit.select_similar",
        "Select Similar Shapes",
        r#"{"type":"object","properties":{"tolerance":{"type":"number","minimum":0,"description":"how far width and height may differ, mm (default 0.1)"}}}"#,
    ),
    cmd(
        "shape.add_star",
        "Add Star",
        r#"{"type":"object","properties":{"x":{"type":"number","description":"centre, mm"},"y":{"type":"number","description":"centre, mm"},"points":{"type":"integer","minimum":3,"maximum":100},"outer":{"type":"number","description":"tip radius, mm"},"inner":{"type":"number","description":"valley radius, mm, below outer"},"rotation":{"type":"number","description":"degrees counter-clockwise; 0 puts a tip straight up"},"layer":{"type":"integer"}},"required":["x","y","points","outer","inner"]}"#,
    ),
    cmd(
        "shape.add_circle_3pt",
        "Add Circle Through Three Points",
        r#"{"type":"object","properties":{"points":{"type":"array","items":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2},"minItems":3,"maxItems":3},"layer":{"type":"integer"}},"required":["points"]}"#,
    ),
    cmd(
        "shape.add_arc_3pt",
        "Add Arc Through Three Points",
        r#"{"type":"object","properties":{"points":{"type":"array","description":"start, a point on the way, end","items":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2},"minItems":3,"maxItems":3},"layer":{"type":"integer"}},"required":["points"]}"#,
    ),
    cmd(
        "shape.open_gap",
        "Leave a Gap",
        r#"{"type":"object","properties":{"length":{"type":"number","exclusiveMinimum":0,"description":"how much of each closed outline is left uncut just before its start point, mm"}},"required":["length"]}"#,
    ),
];

/// The commands of this file (all of them run in this build).
pub fn works(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

/// Width and height may differ by this much (mm) for a shape to count as the same size.
pub const DEFAULT_TOLERANCE: f64 = 0.1;

/// Points closer than this (mm) count as one point.
const MIN_GAP: f64 = 1e-3;

/// How far from the origin (mm) a design can reach: anything further is refused before its
/// outline is built, since a curve that big would take more pieces than memory holds.
const REACH: f64 = darkburn_core::editor::MAX_COORD;

fn in_reach(values: &[f64]) -> Result<(), String> {
    if values.iter().all(|v| v.is_finite() && v.abs() <= REACH) { Ok(()) } else { Err(crate::shape_ops::OUT_OF_REACH.into()) }
}

pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "edit.select_similar" => select_similar(app, a),
        "shape.add_star" => add_star(app, a),
        "shape.add_circle_3pt" => add_circle(app, a),
        "shape.add_arc_3pt" => add_arc(app, a),
        "shape.open_gap" => open_gap(app, a),
        _ => return None,
    })
}

fn num(a: &Value, key: &str) -> Result<f64, String> {
    a.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("argument {key} must be a number"))
}

/// An optional number: absent is None, present but not a finite number is refused.
fn opt_num(a: &Value, key: &str) -> Result<Option<f64>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => num(a, key).map(Some),
    }
}

fn layer(app: &DarkBurn, a: &Value) -> Result<usize, String> {
    match a.get("layer") {
        None | Some(Value::Null) => Ok(app.editor.current_layer),
        Some(v) => {
            let l = v.as_u64().ok_or("argument layer must be a whole number")?;
            if l < darkburn_core::LAYER_COUNT as u64 { Ok(l as usize) } else { Err(format!("layer {l} does not exist (0 to 31)")) }
        }
    }
}

fn add(app: &mut DarkBurn, label: &str, shape: Shape) -> Result<ShapeId, String> {
    let mut new = ShapeId(0);
    let made = app.editor.edit(label, |d, sel| {
        new = d.add(shape);
        sel.set([new]);
        true
    });
    if made { Ok(new) } else { Err(crate::shape_ops::OUT_OF_REACH.into()) }
}

fn select_similar(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let tol = opt_num(a, "tolerance")?.unwrap_or(DEFAULT_TOLERANCE);
    if tol < 0.0 {
        return Err("tolerance must not be below zero".into());
    }
    let chosen = app.editor.selected();
    if chosen.is_empty() {
        return Err("nothing is selected".into());
    }
    let doc = &app.editor.doc;
    let models: Vec<(&'static str, f64, f64)> = chosen
        .iter()
        .filter_map(|id| doc.get(*id))
        .map(|s| {
            let b = s.bbox();
            (s.kind_name(), b.width(), b.height())
        })
        .collect();
    let similar = |s: &Shape| {
        let b = s.bbox();
        models.iter().any(|(k, w, h)| *k == s.kind_name() && (b.width() - w).abs() <= tol && (b.height() - h).abs() <= tol)
    };
    let mut ids = chosen.clone();
    for s in doc.iter() {
        if doc.is_selectable(s) && !ids.contains(&s.id) && similar(s) {
            ids.push(s.id);
        }
    }
    let added = ids.len() - chosen.len();
    app.editor.selection.set(ids.iter().copied());
    Ok(json!({ "selected": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "added": added }))
}

/// The outline of a star: `points` tips at `outer` and valleys at `inner` around `centre`, the
/// first tip straight up turned `rotation` degrees counter-clockwise.
pub fn star_path(centre: Point, points: u32, outer: f64, inner: f64, rotation: f64) -> BezPath {
    let step = std::f64::consts::PI / points as f64;
    let start = std::f64::consts::FRAC_PI_2 + rotation.to_radians();
    let mut p = BezPath::new();
    for k in 0..2 * points {
        let r = if k % 2 == 0 { outer } else { inner };
        let ang = start + step * k as f64;
        let pt = centre + Vec2::new(r * ang.cos(), r * ang.sin());
        if k == 0 { p.move_to(pt) } else { p.line_to(pt) }
    }
    p.close_path();
    p
}

fn add_star(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let (x, y) = (num(a, "x")?, num(a, "y")?);
    let points = a.get("points").and_then(Value::as_u64).ok_or("argument points must be a whole number")?;
    if !(3..=100).contains(&points) {
        return Err("points must be from 3 to 100".into());
    }
    let outer = num(a, "outer")?;
    let inner = num(a, "inner")?;
    in_reach(&[x, y, outer, x.abs() + outer, y.abs() + outer])?;
    if outer < MIN_GAP || inner < MIN_GAP {
        return Err("outer and inner radius must be above zero".into());
    }
    if inner >= outer {
        return Err("inner radius must be smaller than outer radius".into());
    }
    let rotation = opt_num(a, "rotation")?.unwrap_or(0.0);
    let layer = layer(app, a)?;
    let path = star_path(Point::new(x, y), points as u32, outer, inner, rotation);
    let id = add(app, "Add star", Shape::path(path, layer))?;
    Ok(json!({ "id": id.0 }))
}

/// Three distinct points not on one line, from `[[x, y], [x, y], [x, y]]`.
fn three_points(a: &Value) -> Result<[Point; 3], String> {
    let bad = || "argument points must be three [x, y] pairs of numbers".to_string();
    let list = a.get("points").and_then(Value::as_array).filter(|l| l.len() == 3).ok_or_else(bad)?;
    let mut out = [Point::ZERO; 3];
    for (o, v) in out.iter_mut().zip(list) {
        let xy = v.as_array().filter(|p| p.len() == 2).ok_or_else(bad)?;
        let (x, y) = (xy[0].as_f64().ok_or_else(bad)?, xy[1].as_f64().ok_or_else(bad)?);
        if !x.is_finite() || !y.is_finite() {
            return Err(bad());
        }
        *o = Point::new(x, y);
    }
    in_reach(&out.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<_>>())?;
    let [p, q, r] = out;
    if p.distance(q) < MIN_GAP || q.distance(r) < MIN_GAP || p.distance(r) < MIN_GAP {
        return Err("the three points must be apart from each other".into());
    }
    let longest = p.distance(q).max(q.distance(r)).max(p.distance(r));
    if (q - p).cross(r - p).abs() <= 1e-9 * longest * longest {
        return Err("the three points lie on one line".into());
    }
    Ok(out)
}

fn add_circle(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let [p, q, r] = three_points(a)?;
    let layer = layer(app, a)?;
    let (centre, radius) = darkburn_geom::query::circle_through(p, q, r).ok_or("the three points lie on one line")?;
    in_reach(&[radius, centre.x.abs() + radius, centre.y.abs() + radius])?;
    let shape = Shape::ellipse(Rect::from_center_size(centre, (2.0 * radius, 2.0 * radius)), layer);
    let id = add(app, "Add circle", shape)?;
    Ok(json!({ "id": id.0, "centre": [centre.x, centre.y], "radius": radius }))
}

/// The arc from `start` through `mid` to `end`, as curves that begin and end exactly there.
/// None also for a circle beyond [`REACH`].
pub fn arc_path(start: Point, mid: Point, end: Point) -> Option<BezPath> {
    let m = darkburn_geom::query::arc_through(start, mid, end)?;
    in_reach(&[m.radius, m.center.x, m.center.y]).ok()?;
    let arc = Arc::new(m.center, Vec2::new(m.radius, m.radius), (start - m.center).atan2(), m.sweep, 0.0);
    let mut els: Vec<PathEl> = arc.path_elements(1e-4).collect();
    if let Some(PathEl::MoveTo(p)) = els.first_mut() {
        *p = start;
    }
    match els.last_mut() {
        Some(PathEl::CurveTo(_, _, p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p)) => *p = end,
        _ => return None,
    }
    Some(BezPath::from_vec(els))
}

fn add_arc(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let [p, q, r] = three_points(a)?;
    let layer = layer(app, a)?;
    let m = darkburn_geom::query::arc_through(p, q, r).ok_or("the three points lie on one line")?;
    in_reach(&[m.radius, m.center.x, m.center.y])?;
    let path = arc_path(p, q, r).ok_or("the three points lie on one line")?;
    let id = add(app, "Add arc", Shape::path(path, layer))?;
    Ok(json!({ "id": id.0, "centre": [m.center.x, m.center.y], "radius": m.radius, "sweep_degrees": m.sweep.to_degrees() }))
}

fn is_closed(c: &Contour) -> bool {
    c.is_closed_within(1e-6)
}

/// A gap of `length` left in every closed sub-path of `cs` just before its start point: each
/// becomes an open path that starts where it did, runs the same way and stops `length` short of
/// closing. Open sub-paths stay as they are. `Ok(None)` when no sub-path is closed; `Err` with the
/// length of the shortest closed outline when the gap would take all of one.
fn gap_contours(cs: Vec<Contour>, length: f64) -> Result<Option<Vec<Contour>>, f64> {
    if !cs.iter().any(is_closed) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(cs.len());
    for c in cs {
        if !is_closed(&c) {
            out.push(c);
            continue;
        }
        let total = c.length();
        if !total.is_finite() || total - length < MIN_GAP {
            return Err(total);
        }
        let closed = Contour { closed: true, ..c };
        out.push(Contour::new(darkburn_geom::tabs::extract(&closed, 0.0, total - length), false));
    }
    Ok(Some(out))
}

/// The doc-space contours `doc` of shape `s` turned the way the shape is cut: its forced start
/// points and forced direction are put into the outline itself, since an open path has neither.
fn as_cut(s: &Shape, doc: &BezPath) -> Vec<Contour> {
    let mut cs = gpath::contours(doc);
    let own = gpath::contours(&s.outline());
    if own.len() == cs.len() {
        // `own` is the same outline where the shape's parent puts it: the same segments, so a
        // place found on one is the same place on the other.
        for sp in s.start_points() {
            let q = s.transform * Point::new(sp[0], sp[1]);
            let best = own
                .iter()
                .enumerate()
                .filter(|(_, c)| is_closed(c))
                .filter_map(|(i, c)| gpath::nearest_on_contour(c, q).map(|(at, d)| (i, at, d)))
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((i, at, _)) = best {
                let c = Contour { closed: true, ..cs[i].clone() };
                cs[i] = gpath::contour_with_start(&c, at);
            }
        }
    }
    let want = match s.direction {
        CutDirection::Clockwise => Some(Direction::Clockwise),
        CutDirection::CounterClockwise => Some(Direction::CounterClockwise),
        _ => None,
    };
    for c in &mut cs {
        let turn = match want {
            Some(d) => is_closed(c) && !matches!(gpath::contour_direction(c), Direction::Degenerate) && gpath::contour_direction(c) != d,
            None => s.direction == CutDirection::Reversed,
        };
        if turn {
            *c = c.reversed();
        }
    }
    cs
}

/// Forget the forced start points and direction of the shapes `ids` (and of shapes in them).
fn clear_cut_order(s: &mut Shape, ids: &[ShapeId]) {
    if ids.contains(&s.id) {
        s.set_start_points(&[]);
        s.direction = CutDirection::Auto;
    }
    if let ShapeKind::Group { children } = &mut s.kind {
        for c in children {
            clear_cut_order(c, ids);
        }
    }
}

fn open_gap(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let length =
        a.get("length").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).ok_or("argument length must be a number above zero (mm)")?;
    let ids = crate::shape_ops::in_draw_order(&app.editor.doc, &app.editor.selected());
    if ids.is_empty() {
        return Err("nothing is selected".into());
    }
    let mut changed: Vec<ShapeId> = Vec::new();
    let mut skipped: Vec<ShapeId> = Vec::new();
    let mut too_short: Option<f64> = None;
    let done = app.editor.edit("Leave a gap", |d, _| {
        crate::shape_ops::map_shapes_in_doc(d, &ids, &mut |s, p| {
            if too_short.is_some() {
                return None;
            }
            match gap_contours(as_cut(s, p), length) {
                Ok(Some(cs)) => {
                    changed.push(s.id);
                    Some(gpath::from_contours(cs.iter()))
                }
                Ok(None) => {
                    skipped.push(s.id);
                    None
                }
                Err(total) => {
                    too_short = Some(total);
                    None
                }
            }
        });
        for id in &ids {
            if let Some(s) = d.get_mut(*id) {
                clear_cut_order(s, &changed);
            }
        }
        too_short.is_none() && !changed.is_empty()
    });
    if let Some(total) = too_short {
        return Err(format!("the gap must be shorter than the outline it goes in ({total:.3} mm): nothing was cut"));
    }
    if !done {
        return Err(if skipped.is_empty() {
            "the selection has no outline to leave a gap in".into()
        } else {
            "the selected shapes are open already: nothing was cut".into()
        });
    }
    let mut v = json!({ "changed": changed.len(), "ids": changed.iter().map(|i| i.0).collect::<Vec<_>>(), "skipped": skipped.iter().map(|i| i.0).collect::<Vec<_>>() });
    if !skipped.is_empty() {
        v["note"] = json!(format!("{} open shape(s) left as they were", skipped.len()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;
    use darkburn_core::ShapeKind;
    use darkburn_core::kurbo::ParamCurve;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn run(a: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(a, id, &args)
    }

    fn id_of(v: &Value) -> ShapeId {
        ShapeId(v["id"].as_u64().unwrap())
    }

    fn near(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-9
    }

    #[test]
    fn the_commands_are_registered_and_do_not_touch_the_machine() {
        for c in COMMANDS {
            let info = crate::commands::describe(c.id).unwrap_or_else(|| panic!("{} is not registered", c.id));
            assert!(info.implemented && !info.hardware, "{}", c.id);
            assert!(info.args_schema.is_object() && info.args_schema.get("type").is_some(), "{} has a schema", c.id);
        }
    }

    fn rect(a: &mut DarkBurn, x: f64, w: f64, h: f64) -> ShapeId {
        id_of(&run(a, "shape.add_rect", json!({ "x": x, "y": 0, "width": w, "height": h })).unwrap())
    }

    #[test]
    fn select_similar_adds_shapes_of_the_same_kind_and_size() {
        let mut a = app();
        let r1 = rect(&mut a, 0.0, 10.0, 10.0);
        let r2 = rect(&mut a, 20.0, 10.05, 9.95);
        let r3 = rect(&mut a, 40.0, 12.0, 10.0);
        let e = id_of(&run(&mut a, "shape.add_ellipse", json!({ "x": 60, "y": 0, "width": 10, "height": 10 })).unwrap());
        let other = rect(&mut a, 80.0, 5.0, 5.0);
        run(&mut a, "selection.set", json!({ "ids": [r1.0] })).unwrap();
        let before = a.editor.doc.clone();
        let rev = a.editor.revision();
        let v = run(&mut a, "edit.select_similar", Value::Null).unwrap();
        assert_eq!(v["added"], 1);
        assert_eq!(a.editor.selected(), vec![r1, r2]);
        assert_eq!(a.editor.doc, before, "selecting does not change the document");
        assert_eq!(a.editor.revision(), rev, "and leaves nothing to undo");
        // A wider tolerance takes in the 12 mm one; the ellipse is another kind.
        run(&mut a, "edit.select_similar", json!({ "tolerance": 2.5 })).unwrap();
        let sel = a.editor.selected();
        assert!(sel.contains(&r3) && !sel.contains(&e) && !sel.contains(&other), "{sel:?}");
        // Two models: the ellipse and the small rectangle bring in nothing else of theirs.
        run(&mut a, "selection.set", json!({ "ids": [e.0, other.0] })).unwrap();
        let v = run(&mut a, "edit.select_similar", json!({})).unwrap();
        assert_eq!(v["added"], 0);
        assert_eq!(a.editor.selected(), vec![e, other]);
    }

    #[test]
    fn select_similar_refuses_bad_input() {
        let mut a = app();
        rect(&mut a, 0.0, 10.0, 10.0);
        a.editor.selection.clear();
        assert!(run(&mut a, "edit.select_similar", Value::Null).is_err(), "nothing selected");
        run(&mut a, "edit.select_all", Value::Null).unwrap();
        assert!(run(&mut a, "edit.select_similar", json!({ "tolerance": -1 })).is_err());
        assert!(run(&mut a, "edit.select_similar", json!({ "tolerance": "wide" })).is_err());
    }

    #[test]
    fn a_star_has_its_tips_and_valleys_where_asked() {
        let mut a = app();
        a.editor.current_layer = 3;
        let before = a.editor.doc.clone();
        let v = run(&mut a, "shape.add_star", json!({ "x": 50, "y": 40, "points": 5, "outer": 10, "inner": 4 })).unwrap();
        let s = a.editor.doc.get(id_of(&v)).unwrap().clone();
        assert_eq!(s.layer, 3, "the current layer when none is given");
        assert!(s.is_closed());
        assert_eq!(a.editor.selected(), vec![s.id]);
        let els = s.outline().elements().to_vec();
        assert_eq!(els.len(), 11, "move, nine lines, close");
        assert!(matches!(els[10], PathEl::ClosePath));
        let at = |r: f64, deg: f64| Point::new(50.0 + r * deg.to_radians().cos(), 40.0 + r * deg.to_radians().sin());
        for (k, el) in els[..10].iter().enumerate() {
            let p = match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => *p,
                other => panic!("{other:?}"),
            };
            let want = at(if k % 2 == 0 { 10.0 } else { 4.0 }, 90.0 + 36.0 * k as f64);
            assert!(near(p, want), "{k}: {p:?} {want:?}");
        }
        assert!((s.bbox().max_y() - 50.0).abs() < 1e-9);
        // Turned and on a named layer.
        let v = run(&mut a, "shape.add_star", json!({ "x": 0, "y": 0, "points": 4, "outer": 8, "inner": 2, "rotation": 90, "layer": 5 })).unwrap();
        let s = a.editor.doc.get(id_of(&v)).unwrap();
        assert_eq!(s.layer, 5);
        assert!(near(s.outline().elements()[0].end_point().unwrap(), Point::new(-8.0, 0.0)));
        run(&mut a, "edit.undo", Value::Null).unwrap();
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert_eq!(a.editor.doc, before, "undo takes the stars away");
    }

    #[test]
    fn a_star_refuses_bad_input() {
        let mut a = app();
        let ok = json!({ "x": 0, "y": 0, "points": 5, "outer": 10, "inner": 4 });
        let with = |k: &str, v: Value| {
            let mut o = ok.clone();
            o[k] = v;
            o
        };
        for bad in [
            with("points", json!(2)),
            with("points", json!(101)),
            with("points", json!(5.5)),
            with("points", json!(-5)),
            with("outer", json!(0)),
            with("outer", json!(-3)),
            with("inner", json!(0)),
            with("inner", json!(-1)),
            with("inner", json!(10)),
            with("inner", json!(12)),
            with("outer", json!("big")),
            with("rotation", json!("left")),
            with("layer", json!(99)),
            json!({ "x": 0, "points": 5, "outer": 10, "inner": 4 }),
        ] {
            assert!(run(&mut a, "shape.add_star", bad.clone()).is_err(), "{bad}");
        }
        assert!(a.editor.doc.iter().next().is_none(), "nothing was drawn");
        run(&mut a, "shape.add_star", with("points", json!(100))).unwrap();
        run(&mut a, "shape.add_star", with("points", json!(3))).unwrap();
    }

    #[test]
    fn a_circle_through_three_points() {
        let mut a = app();
        a.editor.current_layer = 2;
        let before = a.editor.doc.clone();
        let v = run(&mut a, "shape.add_circle_3pt", json!({ "points": [[60, 50], [50, 60], [40, 50]] })).unwrap();
        let s = a.editor.doc.get(id_of(&v)).unwrap();
        assert_eq!(s.layer, 2);
        match s.kind {
            ShapeKind::Ellipse { rx, ry } => assert!((rx - 10.0).abs() < 1e-9 && (ry - 10.0).abs() < 1e-9),
            ref k => panic!("{k:?}"),
        }
        assert!(near(s.transform * Point::ZERO, Point::new(50.0, 50.0)));
        assert!((v["radius"].as_f64().unwrap() - 10.0).abs() < 1e-9);
        let b = s.bbox();
        assert!((b.x0 - 40.0).abs() < 1e-6 && (b.y1 - 60.0).abs() < 1e-6, "{b:?}");
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert_eq!(a.editor.doc, before);
    }

    fn arc(a: &mut DarkBurn, pts: Value) -> Shape {
        let v = run(a, "shape.add_arc_3pt", json!({ "points": pts })).unwrap();
        a.editor.doc.get(id_of(&v)).unwrap().clone()
    }

    /// Every point along the path is `r` from `c`.
    fn on_circle(p: &BezPath, c: Point, r: f64) {
        for seg in p.segments() {
            for i in 0..=8 {
                let d = seg.eval(i as f64 / 8.0).distance(c);
                assert!((d - r).abs() < 1e-3, "{d}");
            }
        }
    }

    #[test]
    fn an_arc_runs_from_the_first_point_through_the_second_to_the_third() {
        let mut a = app();
        let before = a.editor.doc.clone();
        // Counter-clockwise over the top.
        let s = arc(&mut a, json!([[60, 50], [50, 60], [40, 50]]));
        let p = s.outline();
        assert!(!s.is_closed());
        assert!(near(p.elements()[0].end_point().unwrap(), Point::new(60.0, 50.0)));
        assert!(near(p.elements().last().unwrap().end_point().unwrap(), Point::new(40.0, 50.0)));
        on_circle(&p, Point::new(50.0, 50.0), 10.0);
        let b = p.bounding_box();
        assert!((b.y1 - 60.0).abs() < 1e-3 && (b.y0 - 50.0).abs() < 1e-9, "{b:?}");
        // The same ends the other way round go clockwise, still over the top.
        let p = arc(&mut a, json!([[40, 50], [50, 60], [60, 50]])).outline();
        let b = p.bounding_box();
        assert!((b.y1 - 60.0).abs() < 1e-3 && (b.y0 - 50.0).abs() < 1e-9, "{b:?}");
        // Through the bottom and the left: three quarters of the circle, clockwise.
        let v = run(&mut a, "shape.add_arc_3pt", json!({ "points": [[60, 50], [50, 40], [50, 60]] })).unwrap();
        assert!((v["sweep_degrees"].as_f64().unwrap() + 270.0).abs() < 1e-9);
        let p = a.editor.doc.get(id_of(&v)).unwrap().outline();
        on_circle(&p, Point::new(50.0, 50.0), 10.0);
        let b = p.bounding_box();
        assert!((b.x0 - 40.0).abs() < 1e-3 && (b.y0 - 40.0).abs() < 1e-3 && (b.x1 - 60.0).abs() < 1e-9, "{b:?}");
        assert!(near(p.elements().last().unwrap().end_point().unwrap(), Point::new(50.0, 60.0)));
        assert_eq!(a.editor.doc.get(id_of(&v)).unwrap().layer, a.editor.current_layer);
        for _ in 0..3 {
            run(&mut a, "edit.undo", Value::Null).unwrap();
        }
        assert_eq!(a.editor.doc, before);
    }

    #[test]
    fn three_point_shapes_refuse_bad_points() {
        let mut a = app();
        for id in ["shape.add_circle_3pt", "shape.add_arc_3pt"] {
            for bad in [
                json!({ "points": [[0, 0], [5, 5], [10, 10]] }),
                json!({ "points": [[0, 0], [5, 0], [-7, 0]] }),
                json!({ "points": [[0, 0], [0, 0], [10, 3]] }),
                json!({ "points": [[0, 0], [10, 3], [10, 3]] }),
                json!({ "points": [[1, 1], [5, 2], [1, 1]] }),
                json!({ "points": [[0, 0], [5, 5]] }),
                json!({ "points": [[0, 0], [5, 5], [3, 1], [2, 2]] }),
                json!({ "points": [[0, 0], [5], [3, 1]] }),
                json!({ "points": [[0, 0], ["a", 5], [3, 1]] }),
                json!({ "points": "here" }),
                json!({}),
                json!({ "points": [[60, 50], [50, 60], [40, 50]], "layer": 40 }),
            ] {
                assert!(run(&mut a, id, bad.clone()).is_err(), "{id} {bad}");
            }
        }
        assert!(a.editor.doc.iter().next().is_none(), "nothing was drawn");
    }

    #[test]
    fn shapes_far_beyond_reach_are_refused_before_anything_is_built() {
        // Found by the monkey: an arc through points 1e100 mm out asked kurbo for an outline of
        // more pieces than memory holds, and the app died before the edit could refuse it.
        let mut a = app();
        let far = [1e100, 1e150, 1e300, 2e9];
        for f in far {
            let pts = json!({ "points": [[0, 0], [f, f], [2.0 * f, 0]] });
            for id in ["shape.add_arc_3pt", "shape.add_circle_3pt"] {
                let err = run(&mut a, id, pts.clone()).unwrap_err();
                assert_eq!(err, crate::shape_ops::OUT_OF_REACH, "{id} {pts}");
            }
            for bad in [
                json!({ "x": f, "y": 0, "points": 5, "outer": 10, "inner": 4 }),
                json!({ "x": 0, "y": -f, "points": 5, "outer": 10, "inner": 4 }),
                json!({ "x": 0, "y": 0, "points": 5, "outer": f, "inner": 4 }),
            ] {
                let err = run(&mut a, "shape.add_star", bad.clone()).unwrap_err();
                assert_eq!(err, crate::shape_ops::OUT_OF_REACH, "{bad}");
            }
        }
        // Points in reach, but so nearly on one line that the circle through them is not.
        let flat = json!({ "points": [[0, 0], [5e8, 10], [1e9, 0]] });
        for id in ["shape.add_arc_3pt", "shape.add_circle_3pt"] {
            assert_eq!(run(&mut a, id, flat.clone()).unwrap_err(), crate::shape_ops::OUT_OF_REACH, "{id}");
        }
        assert!(a.editor.doc.iter().next().is_none(), "nothing was drawn");
        // Big but in reach still works.
        run(&mut a, "shape.add_arc_3pt", json!({ "points": [[0, 0], [4e8, 4e8], [8e8, 0]] })).unwrap();
        run(&mut a, "shape.add_star", json!({ "x": 9e8, "y": 0, "points": 5, "outer": 1e7, "inner": 4 })).unwrap();
    }

    // ------------------------------------------------------------ leave a gap

    fn square(a: &mut DarkBurn) -> ShapeId {
        add(a, "square", Shape::path(square_path(), 0)).unwrap()
    }

    /// A 10 mm square from the origin, counter-clockwise: 40 mm round.
    fn square_path() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        p
    }

    fn only_contour(a: &DarkBurn, id: ShapeId) -> Contour {
        let s = a.editor.doc.get(id).unwrap();
        let cs = gpath::contours(&s.outline());
        assert_eq!(cs.len(), 1);
        cs[0].clone()
    }

    fn close(a: Point, b: Point) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn a_gap_is_left_just_before_the_start_and_the_cut_keeps_its_way() {
        let mut a = app();
        let id = square(&mut a);
        let before = a.editor.doc.clone();
        let v = run(&mut a, "shape.open_gap", json!({ "length": 5 })).unwrap();
        assert_eq!(v["changed"], 1);
        let s = a.editor.doc.get(id).unwrap();
        assert!(!s.is_closed(), "the outline is open now");
        let c = only_contour(&a, id);
        assert!(!c.closed);
        assert_eq!(c.nodes().len(), 5, "{:?}", c.nodes());
        assert!(close(c.start().unwrap(), Point::new(0.0, 0.0)), "it starts where it did");
        assert!(close(c.node(1).unwrap(), Point::new(10.0, 0.0)), "and goes the same way");
        assert!(close(c.end().unwrap(), Point::new(0.0, 5.0)), "it stops 5 mm short, on the outline");
        assert!((c.length() - 35.0).abs() < 1e-6);
        assert_eq!(a.editor.selected(), vec![id]);
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert_eq!(a.editor.doc, before);
    }

    #[test]
    fn a_gap_in_a_circle_stays_on_its_outline() {
        // Measured against the shape's own outline: an ellipse is drawn with curves that come
        // close to the true circle but not to within a micrometre.
        let mut a = app();
        let v = run(&mut a, "shape.add_ellipse", json!({ "x": 40, "y": 40, "width": 20, "height": 20 })).unwrap();
        let id = id_of(&v);
        let old = gpath::contours(&a.editor.doc.get(id).unwrap().outline())[0].clone();
        let (start, total) = (old.start().unwrap(), old.length());
        let way = gpath::contour_direction(&old);
        run(&mut a, "shape.open_gap", json!({ "length": 3 })).unwrap();
        let c = only_contour(&a, id);
        assert!(!c.closed);
        assert!(close(c.start().unwrap(), start), "it starts where it did");
        // It ends 3 mm before the start, measured along the old outline.
        let (_, want) = darkburn_geom::tabs::point_at_distance(&old, total - 3.0).unwrap();
        assert!(close(c.end().unwrap(), want), "{:?} {want:?}", c.end());
        assert!((c.length() - (total - 3.0)).abs() < 1e-6, "{} {}", c.length(), total);
        // Every point of the new path lies on the old outline: the gap is cut out, not redrawn.
        for seg in &c.segs {
            for i in 0..=8 {
                let p = seg.eval(i as f64 / 8.0);
                let (_, d2) = gpath::nearest_on_contour(&old, p).unwrap();
                assert!(d2.sqrt() < 1e-6, "{p:?} is {} mm off the outline", d2.sqrt());
            }
        }
        assert_eq!(gpath::contour_direction(&c), way, "same way round");
    }

    #[test]
    fn a_gap_honours_a_forced_start_point_and_direction() {
        let mut a = app();
        let mut sh = Shape::path(square_path(), 0);
        sh.start_point = Some([10.0, 5.0]);
        sh.direction = CutDirection::Clockwise;
        let id = add(&mut a, "square", sh).unwrap();
        run(&mut a, "shape.open_gap", json!({ "length": 5 })).unwrap();
        let c = only_contour(&a, id);
        assert!(close(c.start().unwrap(), Point::new(10.0, 5.0)), "{:?}", c.nodes());
        assert!(close(c.node(1).unwrap(), Point::new(10.0, 0.0)), "clockwise: {:?}", c.nodes());
        assert!(close(c.end().unwrap(), Point::new(10.0, 10.0)), "{:?}", c.nodes());
        assert!((c.length() - 35.0).abs() < 1e-6);
        let s = a.editor.doc.get(id).unwrap();
        assert!(s.start_point.is_none() && s.direction == CutDirection::Auto, "the path itself carries them now");
    }

    #[test]
    fn a_gap_skips_open_shapes_and_one_undo_brings_all_back() {
        let mut a = app();
        let sq = square(&mut a);
        let e = id_of(&run(&mut a, "shape.add_ellipse", json!({ "x": 30, "y": 0, "width": 10, "height": 6 })).unwrap());
        let mut line = BezPath::new();
        line.move_to((0.0, 30.0));
        line.line_to((20.0, 30.0));
        let open = add(&mut a, "line", Shape::path(line, 0)).unwrap();
        run(&mut a, "selection.set", json!({ "ids": [sq.0, e.0, open.0] })).unwrap();
        let before = a.editor.doc.clone();
        let rev = a.editor.revision();
        let v = run(&mut a, "shape.open_gap", json!({ "length": 2 })).unwrap();
        assert_eq!(v["changed"], 2);
        assert_eq!(v["skipped"], json!([open.0]));
        assert!(v["note"].as_str().unwrap().contains("open"));
        assert!(!a.editor.doc.get(sq).unwrap().is_closed() && !a.editor.doc.get(e).unwrap().is_closed());
        assert_eq!(a.editor.doc.get(open).unwrap(), before.get(open).unwrap(), "the open line is left alone");
        assert_eq!(a.editor.revision(), rev + 1, "one step");
        run(&mut a, "edit.undo", Value::Null).unwrap();
        assert_eq!(a.editor.doc, before);
    }

    #[test]
    fn a_gap_refuses_bad_input() {
        let mut a = app();
        let sq = square(&mut a);
        a.editor.selection.clear();
        assert!(run(&mut a, "shape.open_gap", json!({ "length": 5 })).is_err(), "nothing selected");
        run(&mut a, "selection.set", json!({ "ids": [sq.0] })).unwrap();
        let before = a.editor.doc.clone();
        for bad in [
            json!({}),
            json!({ "length": 0 }),
            json!({ "length": -2 }),
            json!({ "length": "5" }),
            json!({ "length": null }),
            json!({ "length": 40 }),
            json!({ "length": 39.9999 }),
            json!({ "length": 55 }),
        ] {
            assert!(run(&mut a, "shape.open_gap", bad.clone()).is_err(), "{bad}");
            assert_eq!(a.editor.doc, before, "{bad} changed nothing");
        }
        // Only open shapes: nothing to cut.
        run(&mut a, "shape.open_gap", json!({ "length": 5 })).unwrap();
        let opened = a.editor.doc.clone();
        let err = run(&mut a, "shape.open_gap", json!({ "length": 5 })).unwrap_err();
        assert!(err.contains("open"), "{err}");
        assert_eq!(a.editor.doc, opened);
        // A gap too long for one of two shapes cuts neither.
        run(&mut a, "edit.undo", Value::Null).unwrap();
        let small = id_of(&run(&mut a, "shape.add_rect", json!({ "x": 50, "y": 0, "width": 2, "height": 2 })).unwrap());
        run(&mut a, "selection.set", json!({ "ids": [sq.0, small.0] })).unwrap();
        let both = a.editor.doc.clone();
        assert!(run(&mut a, "shape.open_gap", json!({ "length": 9 })).is_err());
        assert_eq!(a.editor.doc, both);
    }

    #[test]
    fn the_menus_list_the_new_entries_beside_their_kin() {
        use crate::commands::{Entry, MENUS};
        let edit = MENUS.iter().find(|m| m.0 == "Edit").map(|m| m.1).expect("an Edit menu");
        let ids: Vec<&str> = edit.iter().filter_map(|e| if let Entry::Cmd(id) = e { Some(*id) } else { None }).collect();
        let at = |id: &str| ids.iter().position(|i| *i == id).unwrap_or_else(|| panic!("{id} is not in the Edit menu"));
        assert_eq!(at("shape.open_gap"), at("edit.close_path") + 1);
        assert_eq!(at("edit.select_similar") + 1, at("edit.select_open"));
    }
}
