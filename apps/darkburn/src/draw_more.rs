//! More drawing and selection commands: select shapes like the selected ones, a star, and a
//! circle or an arc through three points. They are document edits and run without a window.

use darkburn_core::kurbo::{Arc, BezPath, PathEl, Point, Rect, Shape as _, Vec2};
use darkburn_core::{Shape, ShapeId};
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
];

/// The commands of this file (all of them run in this build).
pub fn works(id: &str) -> bool {
    COMMANDS.iter().any(|c| c.id == id)
}

/// Width and height may differ by this much (mm) for a shape to count as the same size.
pub const DEFAULT_TOLERANCE: f64 = 0.1;

/// Points closer than this (mm) count as one point.
const MIN_GAP: f64 = 1e-3;

pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "edit.select_similar" => select_similar(app, a),
        "shape.add_star" => add_star(app, a),
        "shape.add_circle_3pt" => add_circle(app, a),
        "shape.add_arc_3pt" => add_arc(app, a),
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
    let shape = Shape::ellipse(Rect::from_center_size(centre, (2.0 * radius, 2.0 * radius)), layer);
    let id = add(app, "Add circle", shape)?;
    Ok(json!({ "id": id.0, "centre": [centre.x, centre.y], "radius": radius }))
}

/// The arc from `start` through `mid` to `end`, as curves that begin and end exactly there.
pub fn arc_path(start: Point, mid: Point, end: Point) -> Option<BezPath> {
    let m = darkburn_geom::query::arc_through(start, mid, end)?;
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
    let path = arc_path(p, q, r).ok_or("the three points lie on one line")?;
    let m = darkburn_geom::query::arc_through(p, q, r).ok_or("the three points lie on one line")?;
    let id = add(app, "Add arc", Shape::path(path, layer))?;
    Ok(json!({ "id": id.0, "centre": [m.center.x, m.center.y], "radius": m.radius, "sweep_degrees": m.sweep.to_degrees() }))
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
}
