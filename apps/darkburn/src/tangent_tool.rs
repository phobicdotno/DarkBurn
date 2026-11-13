//! Tangent circles (DRAW-22): pick two lines, arcs or circles and a radius, or three of them,
//! and get every circle that touches all of them. The geometry is `darkburn_geom::tangent`;
//! this file is the canvas tool (click edges, choose among the dashed candidates in a small
//! window), its preview, and the `shape.tangent_circle` command that agents and scripts use.
//!
//! How it behaves is this project's own design; the checklist only names the feature:
//! - Clicking near a straight edge, an arc or a circle of any shape picks it (an edge of a
//!   rectangle or polygon counts on its own); clicking a picked one again drops it, and a click
//!   after three picks starts over.
//! - Two picks give the circles of the radius in the window, three give the circles that touch
//!   all three (up to eight). Every candidate is drawn dashed; the chosen one is drawn solid.
//!   Clicking a dashed circle chooses it.
//! - By default only circles that touch the picked segments and arcs themselves are offered,
//!   not their extensions; a checkbox shows the rest. The candidates nearest the clicks come
//!   first.
//! - Add puts the chosen circle, Add All every candidate, on the current layer as one undo
//!   step. The picks are then cleared and the tool stays on.

use darkburn_core::kurbo::{BezPath, Circle, Point, Rect, Shape as _};
use darkburn_core::{Document, Shape, ShapeId};
use darkburn_geom::tangent::{self, Element, Solution, TangentError};
use darkburn_ui::ui_kit;
use egui::{Color32, Stroke};
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::numeric::{self, Num};
use crate::render::{self, Xf};
use crate::shell::DarkBurn;
use crate::tools::Tool;

/// The command that picks the tool.
pub const TOOL: &str = "tools.tangent_circle";
/// The command that adds tangent circles from points or the selection.
pub const ADD: &str = "shape.tangent_circle";

/// The status bar hint while the tool is active.
pub const HINT: &str = "Click two lines, arcs or circles for circles of the radius in the window, or three for the circles touching all of them. Click a dashed circle to choose it.";

/// How far (mm) a curve may stray from a circle and still count as an arc.
const FIT_TOL: f64 = 0.05;
/// How closely (mm) a result must touch what it was picked against.
const CHECK_TOL: f64 = 1e-4;
/// The radius the tool starts with (mm).
pub const DEFAULT_RADIUS: f64 = 10.0;
/// Larger results are left out: they would fit no bed (mm).
const MAX_RADIUS: f64 = 100_000.0;

/// Whether `id` is one of ours.
pub fn handles(id: &str) -> bool {
    id == TOOL || id == ADD
}

// ================================================================ pure parts

/// The lines, arcs and circles of the shapes `ids` (every shape when `None`). With `near`, only
/// shapes whose box comes within the distance of the point are read. Parts on hidden layers are
/// left out, as nobody can see what they would touch; locked shapes count, since they are only
/// read here.
pub fn elements_of(doc: &Document, ids: Option<&[ShapeId]>, near: Option<(Point, f64)>) -> Vec<Element> {
    let mut out = Vec::new();
    for s in doc.iter() {
        if ids.is_some_and(|ids| !ids.contains(&s.id)) {
            continue;
        }
        if let Some((p, d)) = near
            && !s.bbox().inflate(d, d).contains(p)
        {
            continue;
        }
        for part in crate::shape_ops::parts_of(doc, s.id) {
            if !doc.layer(part.layer).show {
                continue;
            }
            out.extend(tangent::elements(&part.path, FIT_TOL));
        }
    }
    out
}

/// The element nearest `p` and its distance.
pub fn nearest(els: &[Element], p: Point) -> Option<(Element, f64)> {
    els.iter().map(|e| (*e, e.distance(p))).filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The circles touching every one of `picks`: two picks need `radius`, three need none.
pub fn solve(picks: &[Element], radius: f64) -> Result<Vec<Solution>, TangentError> {
    let mut out = match picks {
        [a, b] => tangent::with_radius(a, b, radius, CHECK_TOL)?,
        [a, b, c] => tangent::apollonius(a, b, c, CHECK_TOL)?,
        _ => return Err(TangentError::BadInput),
    };
    out.retain(|s| s.circle.radius <= MAX_RADIUS);
    if out.is_empty() { Err(TangentError::NoSolution) } else { Ok(out) }
}

/// What to tell the person when there is no circle.
pub fn message(e: TangentError) -> String {
    match e {
        TangentError::BadRadius => tr("tangent.bad_radius", "The radius must be above zero."),
        TangentError::BadInput => tr("tangent.bad_input", "Pick two or three straight lines, arcs or circles."),
        TangentError::Endless => tr("tangent.endless", "Endless circles touch these, so there is none to pick. Pick something else."),
        TangentError::NoSolution => tr("tangent.none", "No circle touches all of these."),
    }
}

/// The circles to offer for `picks`: with `on_edges` only those touching the segments and arcs
/// themselves. When there is a click point per pick, the circles touching nearest the clicks
/// come first.
pub fn candidates(picks: &[Element], clicks: &[Point], radius: f64, on_edges: bool) -> Result<Vec<Solution>, String> {
    let mut all = solve(picks, radius).map_err(message)?;
    if on_edges {
        all.retain(|s| s.on_elements);
        if all.is_empty() {
            return Err(tr(
                "tangent.off_edges",
                "No circle touches the picked edges themselves. Untick Only on the edges to see the ones touching their extensions.",
            ));
        }
    }
    if !clicks.is_empty() && clicks.len() == picks.len() {
        let score = |s: &Solution| s.touches.iter().zip(clicks).map(|(t, c)| t.distance(*c)).sum::<f64>();
        all.sort_by(|a, b| score(a).total_cmp(&score(b)));
    }
    Ok(all)
}

/// Add `circles` on `layer` as one undo step and select them.
pub fn add_circles(app: &mut DarkBurn, circles: &[Circle], layer: usize) -> Result<Vec<ShapeId>, String> {
    let mut ids = Vec::new();
    let done = app.editor.edit("Tangent circle", |d, sel| {
        for c in circles {
            let d2 = c.radius * 2.0;
            ids.push(d.add(Shape::ellipse(Rect::from_center_size(c.center, (d2, d2)), layer)));
        }
        sel.set(ids.iter().copied());
        !ids.is_empty()
    });
    if done { Ok(ids) } else { Err(tr("tangent.not_added", "The circle was not added: it reaches too far.")) }
}

/// A short description of a picked element, for the window.
fn describe(e: &Element) -> String {
    match *e {
        Element::Segment(l) => format!("{}, {:.2} mm", tr("tangent.line", "Line"), l.p0.distance(l.p1)),
        Element::Arc { radius, .. } => format!("{}, R {:.2} mm", tr("tangent.arc", "Arc"), radius),
        Element::Circle(c) => format!("{}, R {:.2} mm", tr("tangent.circle", "Circle"), c.radius),
    }
}

// ================================================================ agent command

/// The agent path for both commands.
pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    match id {
        TOOL => {
            app.set_tool(Tool::TangentCircle);
            Some(Ok(json!({ "changed": false, "tool": "tangent_circle" })))
        }
        ADD => Some(add_command(app, args)),
        _ => None,
    }
}

fn points(v: &Value) -> Result<Vec<Point>, String> {
    let bad = || "argument points must be a list of [x, y] pairs".to_string();
    let list = v.as_array().ok_or_else(bad)?;
    list.iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([x, y]) => match (x.as_f64(), y.as_f64()) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Point::new(x, y)),
                _ => Err(bad()),
            },
            _ => Err(bad()),
        })
        .collect()
}

fn flag(a: &Value, name: &str, default: bool) -> Result<bool, String> {
    match a.get(name) {
        None => Ok(default),
        Some(v) => v.as_bool().ok_or_else(|| format!("argument {name} must be true or false")),
    }
}

fn layer_arg(a: &Value, default: usize) -> Result<usize, String> {
    let Some(v) = a.get("layer") else { return Ok(default) };
    let l = v.as_u64().ok_or("argument layer must be a whole number")?;
    let count = darkburn_core::LAYER_COUNT as u64;
    if l < count { Ok(l as usize) } else { Err(format!("layer {l} does not exist (0 to {})", count - 1)) }
}

/// `shape.tangent_circle`: the elements come from `points` (each picks the nearest line, arc or
/// circle among the selection, or among every shape when nothing is selected) or else from the
/// selection, which must hold exactly two or three.
fn add_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let selected = app.editor.selected();
    let (picks, clicks) = match a.get("points") {
        Some(v) => {
            let pts = points(v)?;
            if !(2..=3).contains(&pts.len()) {
                return Err("argument points must hold two or three [x, y] points".into());
            }
            let scope = if selected.is_empty() { None } else { Some(selected.as_slice()) };
            let els = elements_of(&app.editor.doc, scope, None);
            let mut picks = Vec::new();
            for p in &pts {
                let (e, _) = nearest(&els, *p).ok_or_else(|| tr("tangent.no_elements", "There is no straight line, arc or circle to pick."))?;
                picks.push(e);
            }
            (picks, pts)
        }
        None => {
            let els = elements_of(&app.editor.doc, Some(&selected), None);
            if !(2..=3).contains(&els.len()) {
                return Err(tr("tangent.select_some", "Select shapes made of two or three lines, arcs or circles, or give points to pick them."));
            }
            (els, Vec::new())
        }
    };
    let radius = match a.get("radius") {
        Some(v) => v.as_f64().ok_or("argument radius must be a number")?,
        None if picks.len() == 3 => 0.0,
        None => return Err("argument radius is needed for two objects".into()),
    };
    let on_edges = flag(a, "on_edges", true)?;
    let preview = flag(a, "preview", false)?;
    let layer = layer_arg(a, app.editor.current_layer)?;
    let sols = candidates(&picks, &clicks, radius, on_edges)?;
    let list: Vec<Value> = sols
        .iter()
        .map(|s| json!({ "x": s.circle.center.x, "y": s.circle.center.y, "radius": s.circle.radius, "on_elements": s.on_elements }))
        .collect();
    if preview {
        return Ok(json!({ "solutions": list, "added": [], "changed": false }));
    }
    let chosen: Vec<Circle> = match a.get("pick") {
        None => vec![sols[0].circle],
        Some(Value::String(s)) if s == "all" => sols.iter().map(|s| s.circle).collect(),
        Some(v) => {
            let i = v.as_u64().ok_or("argument pick must be an index or \"all\"")? as usize;
            vec![sols.get(i).ok_or_else(|| format!("pick {i} is out of range: there are {} circles", sols.len()))?.circle]
        }
    };
    let ids = add_circles(app, &chosen, layer)?;
    Ok(json!({ "solutions": list, "added": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "changed": true }))
}

// ================================================================ the canvas tool

/// What the tool has picked so far, kept with the window between frames.
#[derive(Clone, Debug)]
pub struct State {
    /// The picked elements and where each was clicked.
    pub picks: Vec<(Element, Point)>,
    pub radius: f64,
    pub on_edges: bool,
    /// The candidate Add puts in.
    pub chosen: usize,
    pub message: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State { picks: Vec::new(), radius: DEFAULT_RADIUS, on_edges: true, chosen: 0, message: None }
    }
}

impl State {
    /// The candidates for the picks so far: none until two are picked.
    pub fn candidates(&self) -> Result<Vec<Solution>, String> {
        if self.picks.len() < 2 {
            return Ok(Vec::new());
        }
        let els: Vec<Element> = self.picks.iter().map(|(e, _)| *e).collect();
        let clicks: Vec<Point> = self.picks.iter().map(|(_, p)| *p).collect();
        candidates(&els, &clicks, self.radius, self.on_edges)
    }

    /// A click at `p` (mm), `tol` being the pick distance: choose the candidate circle there, or
    /// pick or drop the line, arc or circle there.
    pub fn click(&mut self, doc: &Document, p: Point, tol: f64) {
        let els = elements_of(doc, None, Some((p, tol)));
        let near = nearest(&els, p).filter(|(_, d)| *d <= tol);
        let cands = self.candidates().unwrap_or_default();
        let circle = cands
            .iter()
            .enumerate()
            .map(|(i, s)| (i, (p.distance(s.circle.center) - s.circle.radius).abs()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, d)| *d <= tol);
        if let Some((i, d)) = circle
            && near.is_none_or(|(_, e)| d < e)
        {
            self.chosen = i;
            self.message = None;
            return;
        }
        let Some((e, _)) = near else {
            self.message = Some(tr("tangent.nothing_here", "Nothing to pick here: click a straight line, an arc or a circle."));
            return;
        };
        self.message = None;
        self.chosen = 0;
        if let Some(i) = self.picks.iter().position(|(q, _)| *q == e) {
            self.picks.remove(i);
            return;
        }
        if self.picks.len() >= 3 {
            self.picks.clear();
        }
        self.picks.push((e, p));
    }

    /// Forget the picks (the radius and the edges option stay).
    pub fn clear(&mut self) {
        self.picks.clear();
        self.chosen = 0;
        self.message = None;
    }
}

fn state_id() -> egui::Id {
    egui::Id::unique("db_tangent_tool")
}

/// The tool's state (the default when it has none yet).
pub fn state(ctx: &egui::Context) -> State {
    ctx.data(|d| d.get_temp::<State>(state_id())).unwrap_or_default()
}

fn store(ctx: &egui::Context, st: State) {
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

/// Run from a menu, key or the tools: pick the tool. `shape.tangent_circle` also picks what
/// the selection holds when that is two or three lines, arcs or circles.
pub fn run(app: &mut DarkBurn, ctx: &egui::Context, id: &str) -> bool {
    if !handles(id) {
        return false;
    }
    app.set_tool(Tool::TangentCircle);
    let mut st = state(ctx);
    st.clear();
    if id == ADD {
        let selected = app.editor.selected();
        let els = elements_of(&app.editor.doc, Some(&selected), None);
        if (2..=3).contains(&els.len()) {
            let middle = app.editor.doc.bbox_of(&selected).map(|r| r.center()).unwrap_or_default();
            st.picks = els.iter().map(|e| (*e, e.nearest(middle))).collect();
        } else if !selected.is_empty() {
            st.message = Some(tr("tangent.select_some", "Select shapes made of two or three lines, arcs or circles, or give points to pick them."));
        }
    }
    store(ctx, st);
    true
}

/// The tool's click on the canvas.
pub fn interact(ui: &egui::Ui, app: &mut DarkBurn, response: &egui::Response, xf: &Xf, raw: Option<Point>) {
    let Some(p) = raw else { return };
    if !response.clicked_by(egui::PointerButton::Primary) {
        return;
    }
    let mut st = state(ui.ctx());
    st.click(&app.editor.doc, p, xf.mm(crate::options::get().click_tolerance_px));
    store(ui.ctx(), st);
}

fn add_from_window(app: &mut DarkBurn, st: &mut State, circles: &[Circle]) {
    if circles.is_empty() {
        return;
    }
    let layer = app.editor.current_layer;
    match add_circles(app, circles, layer) {
        Ok(_) => st.clear(),
        Err(m) => st.message = Some(m),
    }
}

/// The tool's window, every frame: shown while the tool is on. Closing it (or Esc) goes back to
/// the select tool.
pub fn window(ctx: &egui::Context, app: &mut DarkBurn) {
    if app.tool != Tool::TangentCircle {
        if let Some(mut st) = ctx.data(|d| d.get_temp::<State>(state_id()))
            && (!st.picks.is_empty() || st.message.is_some())
        {
            st.clear();
            store(ctx, st);
        }
        return;
    }
    let mut st = state(ctx);
    let cands = st.candidates();
    let count = cands.as_ref().map_or(0, Vec::len);
    if st.chosen >= count {
        st.chosen = 0;
    }
    let title = tr("tangent.title", "Tangent Circle");
    let add = tr("tangent.add", "Add Circle");
    let add_all = tr("tangent.add_all", "Add All");
    let close = tr("ui.close", "Close");
    let room = crate::shell::room_under_the_bar(ctx);
    let pos = room.left_top() + egui::vec2(crate::shell::TOOLS_WIDTH + 24.0, 24.0);
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_tangent"), "ellipse", &title)
        .fields()
        .actions(&add, &close, Some(&add_all))
        .apply_enabled(count > 1)
        .placed(egui::Align2::LEFT_TOP, pos)
        .show(ctx, count > 0, |ui| body(ui, &mut st, &cands));
    match action {
        ui_kit::Action::Ok => {
            let one: Vec<Circle> = cands.iter().flatten().nth(st.chosen).map(|s| s.circle).into_iter().collect();
            add_from_window(app, &mut st, &one);
        }
        ui_kit::Action::Apply => {
            let all: Vec<Circle> = cands.iter().flatten().map(|s| s.circle).collect();
            add_from_window(app, &mut st, &all);
        }
        _ => {}
    }
    if closed || action == ui_kit::Action::Cancel {
        app.set_tool(Tool::Select);
        st.clear();
    }
    store(ctx, st);
}

fn body(ui: &mut egui::Ui, st: &mut State, cands: &Result<Vec<Solution>, String>) {
    ui.set_min_width(300.0);
    let help = match st.picks.len() {
        0 | 1 => tr(
            "tangent.help_pick",
            "Click two lines, arcs or circles for the circles of the radius below, or three for the circles that touch all of them.",
        ),
        2 => tr("tangent.help_two", "Click a dashed circle to choose it, or click a third object for the circles touching all three."),
        _ => tr("tangent.help_three", "Click a dashed circle to choose it. Click a picked object again to drop it."),
    };
    ui.add(egui::Label::new(help).wrap());
    let mut drop = None;
    for (i, (e, _)) in st.picks.iter().enumerate() {
        if ui_kit::list_row(ui, &format!("tangent_pick_{i}"), &describe(e)).clicked() {
            drop = Some(i);
        }
    }
    if let Some(i) = drop {
        st.picks.remove(i);
        st.chosen = 0;
    }
    let radius = tr("tangent.radius", "Radius");
    ui.push_id("tangent_radius", |ui| numeric::row(ui, &radius, &mut st.radius, &Num::new(0.01..=10_000.0, 2, " mm")));
    ui_kit::checkbox(ui, &tr("tangent.on_edges", "Only on the edges"), &mut st.on_edges);
    match cands {
        Ok(list) => {
            for (i, s) in list.iter().enumerate() {
                if ui.selectable_label(i == st.chosen, row_text(i, s)).clicked() {
                    st.chosen = i;
                }
            }
        }
        Err(m) => {
            ui.add(egui::Label::new(m.as_str()).wrap());
        }
    }
    if let Some(m) = &st.message {
        ui.add(egui::Label::new(m.as_str()).wrap());
    }
}

/// The text of candidate `i` in the window's list, in the interface language.
pub fn row_text(i: usize, s: &Solution) -> String {
    let mut text = tr("tangent.row", "Circle {n}: R {r} mm at {x}, {y}")
        .replace("{n}", &(i + 1).to_string())
        .replace("{r}", &format!("{:.2}", s.circle.radius))
        .replace("{x}", &format!("{:.2}", s.circle.center.x))
        .replace("{y}", &format!("{:.2}", s.circle.center.y));
    if !s.on_elements {
        text = format!("{text} ({})", tr("tangent.on_extension", "touches an extension"));
    }
    text
}

/// The preview on the canvas: the picks drawn thick, every candidate dashed, the chosen one
/// solid.
pub fn paint_overlay(ctx: &egui::Context, app: &DarkBurn, xf: &Xf, painter: &egui::Painter, accent: Color32) {
    if app.tool != Tool::TangentCircle {
        return;
    }
    let Some(st) = ctx.data(|d| d.get_temp::<State>(state_id())) else { return };
    let draw = |path: &BezPath, stroke: Stroke, dashed: bool| {
        for (mut pts, closed) in render::screen_polylines(path, xf, true) {
            if closed && let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            if dashed {
                painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
            } else {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    };
    for (e, _) in &st.picks {
        draw(&e.path(0.01), Stroke::new(3.0, accent), false);
    }
    if let Ok(list) = st.candidates() {
        for (i, s) in list.iter().enumerate() {
            let path = s.circle.to_path(0.01);
            if i == st.chosen {
                draw(&path, Stroke::new(2.0, accent), false);
            } else {
                draw(&path, Stroke::new(1.0, accent.gamma_multiply(0.7)), true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;
    use darkburn_core::kurbo::Line;

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    /// A 40 x 30 rectangle with its lower left corner at (10, 10).
    fn with_rect() -> DarkBurn {
        let mut a = DarkBurn::new(Prefs::default());
        run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 30, "layer": 0 })).unwrap();
        a.editor.selection.clear();
        a
    }

    fn circles(app: &DarkBurn) -> Vec<Circle> {
        app.editor
            .doc
            .iter()
            .filter_map(|s| match s.kind {
                darkburn_core::ShapeKind::Ellipse { rx, ry } if (rx - ry).abs() < 1e-9 => Some(Circle::new(s.bbox().center(), rx)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_corner_of_a_rectangle_gets_its_fillet_circle_in_one_undo_step() {
        let mut a = with_rect();
        let undo_before = a.editor.history.len();
        let out = run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5 })).unwrap();
        assert_eq!(out["changed"], true);
        // Only the circle inside the corner touches both edges themselves.
        assert_eq!(out["solutions"].as_array().unwrap().len(), 1, "{out}");
        let c = circles(&a);
        assert_eq!(c.len(), 1);
        assert!(c[0].center.distance(Point::new(15.0, 15.0)) < 1e-6 && (c[0].radius - 5.0).abs() < 1e-6, "{c:?}");
        assert_eq!(a.editor.history.len(), undo_before + 1);
        assert_eq!(a.editor.selected(), vec![ShapeId(out["added"][0].as_u64().unwrap())]);
        assert_eq!(a.editor.doc.get(a.editor.selected()[0]).unwrap().layer, a.editor.current_layer);
        run(&mut a, "edit.undo", json!({})).unwrap();
        assert!(circles(&a).is_empty());
    }

    #[test]
    fn extensions_add_the_other_three_corner_circles() {
        let mut a = with_rect();
        let out = run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5, "on_edges": false, "preview": true })).unwrap();
        assert_eq!(out["changed"], false);
        let list = out["solutions"].as_array().unwrap();
        assert_eq!(list.len(), 4);
        // Nearest the clicks first: the one inside the corner.
        assert_eq!((list[0]["x"].as_f64().unwrap().round(), list[0]["y"].as_f64().unwrap().round()), (15.0, 15.0));
        assert!(circles(&a).is_empty(), "a preview adds nothing");
        let undo_before = a.editor.history.len();
        let out = run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5, "on_edges": false, "pick": "all", "layer": 3 })).unwrap();
        assert_eq!(out["added"].as_array().unwrap().len(), 4);
        assert!(a.editor.doc.iter().filter(|s| s.layer == 3).count() == 4);
        assert_eq!(a.editor.history.len(), undo_before + 1, "all four circles are one undo step");
        run(&mut a, "edit.undo", json!({})).unwrap();
        assert!(circles(&a).is_empty(), "one undo takes all four away");
    }

    #[test]
    fn shapes_on_hidden_layers_are_not_picked() {
        // A rectangle on layer 1, then layer 1 hidden; a visible line on layer 0 farther away.
        let mut a = DarkBurn::new(Prefs::default());
        run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 30, "layer": 1 })).unwrap();
        let mut line = BezPath::new();
        line.move_to((0.0, 60.0));
        line.line_to((100.0, 60.0));
        a.editor.doc.add(Shape::path(line, 0));
        a.editor.selection.clear();
        assert_eq!(elements_of(&a.editor.doc, None, None).len(), 5);
        a.editor.doc.layer_mut(1).show = false;
        let els = elements_of(&a.editor.doc, None, None);
        assert_eq!(els.len(), 1, "only the visible line is left: {els:?}");
        let Element::Segment(l) = els[0] else { panic!("{els:?}") };
        assert!(l.p0.distance(Point::new(0.0, 60.0)) < 1e-9 && l.p1.distance(Point::new(100.0, 60.0)) < 1e-9, "{l:?}");

        // On the canvas: a click on the hidden rectangle's edge picks nothing.
        let mut st = State { radius: 5.0, ..State::default() };
        st.click(&a.editor.doc, Point::new(10.0, 20.0), 0.5);
        assert!(st.picks.is_empty() && st.message.is_some(), "{st:?}");

        // Through the command: points on the hidden edges fall to the visible line, which
        // cannot give a pair; nothing is added.
        let out = run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5 }));
        assert!(out.is_err(), "{out:?}");
        assert!(circles(&a).is_empty());

        // Shown again, the same command works.
        a.editor.doc.layer_mut(1).show = true;
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5 })).is_ok());
    }

    #[test]
    fn a_large_circle_from_an_svg_file_can_be_picked() {
        // An SVG drawn in mm: usvg writes the circle as one cubic per quarter.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="600mm" height="600mm" viewBox="0 0 600 600"><circle cx="300" cy="300" r="200" fill="none" stroke="black"/></svg>"#;
        let imported = crate::import::svg(svg, &crate::import::Options::default()).unwrap();
        let mut a = DarkBurn::new(Prefs::default());
        for s in imported.shapes {
            a.editor.doc.add(s);
        }
        let els = elements_of(&a.editor.doc, None, None);
        assert_eq!(els.len(), 1, "{els:?}");
        let Element::Circle(c) = els[0] else { panic!("{els:?}") };
        assert!((c.radius - 200.0).abs() < 0.01, "{c:?}");
        let mut st = State::default();
        st.click(&a.editor.doc, c.center + darkburn_core::kurbo::Vec2::new(200.2, 0.0), 0.5);
        assert_eq!(st.picks.len(), 1, "{st:?}");
    }

    #[test]
    fn candidate_rows_are_translated_whole() {
        let s = Solution { circle: Circle::new((15.0, 15.0), 10.0), touches: Vec::new(), on_elements: false };
        assert_eq!(row_text(0, &s), "Circle 1: R 10.00 mm at 15.00, 15.00 (touches an extension)");
        crate::i18n::set_language("nb");
        let nb = row_text(0, &s);
        crate::i18n::set_language("en");
        assert!(nb.starts_with("Sirkel 1: R 10.00 mm") && nb.contains("15.00, 15.00"), "{nb}");
        assert!(!nb.contains(" at ") && !nb.contains("touches"), "{nb}");
    }

    #[test]
    fn three_edges_give_the_circle_touching_all_three() {
        let mut a = with_rect();
        // Left, bottom and right edges, 40 apart: the circle of radius 20 standing on the bottom
        // edge is the only one touching all three edges themselves.
        let out = run(&mut a, ADD, json!({ "points": [[10, 20], [30, 10], [50, 20]] })).unwrap();
        assert_eq!(out["solutions"].as_array().unwrap().len(), 1, "{out}");
        let c = circles(&a);
        assert_eq!(c.len(), 1);
        assert!(c[0].center.distance(Point::new(30.0, 30.0)) < 1e-6 && (c[0].radius - 20.0).abs() < 1e-6, "{c:?}");
    }

    #[test]
    fn a_selection_of_two_circles_and_a_radius_works_without_points() {
        let mut a = DarkBurn::new(Prefs::default());
        let c1 = run(&mut a, "shape.add_ellipse", json!({ "x": 0, "y": 0, "width": 20, "height": 20 })).unwrap();
        let c2 = run(&mut a, "shape.add_ellipse", json!({ "x": 40, "y": 0, "width": 20, "height": 20 })).unwrap();
        run(&mut a, "selection.set", json!({ "ids": [c1["id"], c2["id"]] })).unwrap();
        let out = run(&mut a, ADD, json!({ "radius": 20, "preview": true })).unwrap();
        let list = out["solutions"].as_array().unwrap();
        assert!(!list.is_empty() && list.len() <= 8, "{out}");
        for s in list {
            assert!((s["radius"].as_f64().unwrap() - 20.0).abs() < 1e-6);
            let d = |cx: f64| (s["x"].as_f64().unwrap() - cx).hypot(s["y"].as_f64().unwrap() - 10.0);
            for cx in [10.0, 50.0] {
                assert!((d(cx) - 30.0).abs() < 1e-3 || (d(cx) - 10.0).abs() < 1e-3, "{s} against the circle at {cx}");
            }
        }
    }

    #[test]
    fn bad_input_gives_plain_messages() {
        let mut a = with_rect();
        assert!(run(&mut a, ADD, json!({})).unwrap_err().contains("two or three"));
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20]] })).unwrap_err().contains("two or three"));
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]] })).unwrap_err().contains("radius"));
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": -1 })).unwrap_err().contains("radius"));
        assert!(run(&mut a, ADD, json!({ "points": "here", "radius": 1 })).is_err());
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5, "pick": 7 })).unwrap_err().contains("out of range"));
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [20, 10]], "radius": 5, "layer": 99 })).is_err());
        // The same edge twice: endless circles, no list.
        assert!(run(&mut a, ADD, json!({ "points": [[10, 20], [10, 25]], "radius": 5 })).unwrap_err().contains("Endless"));
        // Opposite sides 30 apart cannot both touch a circle of radius 5.
        assert!(run(&mut a, ADD, json!({ "points": [[20, 10], [20, 40]], "radius": 5 })).is_err());
        // An empty drawing.
        let mut b = DarkBurn::new(Prefs::default());
        assert!(run(&mut b, ADD, json!({ "points": [[0, 0], [1, 1]], "radius": 1 })).is_err());
        assert!(circles(&a).is_empty() && circles(&b).is_empty());
    }

    #[test]
    fn the_tool_command_picks_the_tool() {
        let mut a = DarkBurn::new(Prefs::default());
        run(&mut a, TOOL, json!({})).unwrap();
        assert_eq!(a.tool, Tool::TangentCircle);
        assert_eq!(Tool::TangentCircle.command(), TOOL);
        assert!(crate::commands::find(TOOL).is_some() && crate::commands::find(ADD).is_some());
        assert_eq!(crate::commands::checklist(TOOL), &["DRAW-22"]);
        assert!(!crate::commands::hardware(TOOL) && !crate::commands::hardware(ADD));
    }

    #[test]
    fn clicks_pick_drop_and_choose() {
        let a = with_rect();
        let doc = &a.editor.doc;
        let mut st = State { radius: 5.0, ..State::default() };
        st.click(doc, Point::new(30.0, 25.0), 0.5);
        assert!(st.picks.is_empty() && st.message.is_some(), "nothing near the middle");
        st.click(doc, Point::new(10.2, 20.0), 0.5);
        st.click(doc, Point::new(20.0, 9.9), 0.5);
        assert_eq!(st.picks.len(), 2);
        assert!(matches!(st.picks[0].0, Element::Segment(_)));
        assert_eq!(st.candidates().unwrap().len(), 1);
        st.on_edges = false;
        let all = st.candidates().unwrap();
        assert_eq!(all.len(), 4);
        assert!(all[0].circle.center.distance(Point::new(15.0, 15.0)) < 1e-6, "the one in the corner comes first");
        // Clicking a dashed circle chooses it: the one outside the corner, clicked on its left.
        let far = all.iter().position(|s| s.circle.center.distance(Point::new(5.0, 5.0)) < 1e-6).unwrap();
        st.click(doc, Point::new(0.0, 5.0), 0.5);
        assert_eq!(st.chosen, far);
        assert_eq!(st.picks.len(), 2);
        // Clicking a picked edge again drops it.
        st.click(doc, Point::new(10.0, 30.0), 0.5);
        assert_eq!(st.picks.len(), 1);
        assert_eq!(st.chosen, 0);
        // Three picks, then a fourth click starts over.
        st.click(doc, Point::new(10.0, 30.0), 0.5);
        st.click(doc, Point::new(50.0, 30.0), 0.5);
        assert_eq!(st.picks.len(), 3);
        st.click(doc, Point::new(30.0, 40.0), 0.5);
        assert_eq!(st.picks.len(), 1);
    }

    #[test]
    fn arcs_and_circles_of_drawn_shapes_are_picked_as_such() {
        let mut a = DarkBurn::new(Prefs::default());
        run(&mut a, "shape.add_ellipse", json!({ "x": 0, "y": 0, "width": 40, "height": 40 })).unwrap();
        let els = elements_of(&a.editor.doc, None, None);
        assert_eq!(els.len(), 1);
        let Element::Circle(c) = els[0] else { panic!("{els:?}") };
        assert!(c.center.distance(Point::new(20.0, 20.0)) < 0.01 && (c.radius - 20.0).abs() < 0.01, "{c:?}");
        let near = nearest(&els, Point::new(41.0, 20.0)).unwrap();
        assert!((near.1 - 1.0).abs() < 0.01);
        let line = Element::Segment(Line::new((0.0, -5.0), (40.0, -5.0)));
        assert_eq!(solve(&[els[0], line], 5.0).unwrap().len(), 2, "two circles of radius 5 rest on the line against the circle");
        assert_eq!(solve(&[els[0]], 5.0), Err(TangentError::BadInput));
    }

    #[test]
    fn leaving_the_tool_forgets_the_picks() {
        let ctx = egui::Context::default();
        let mut a = with_rect();
        assert!(run_ui(&ctx, &mut a, ADD));
        assert_eq!(a.tool, Tool::TangentCircle);
        let mut st = state(&ctx);
        st.click(&a.editor.doc, Point::new(10.0, 20.0), 0.5);
        store(&ctx, st);
        a.set_tool(Tool::Select);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(ui.ctx(), &mut a));
        out.textures_delta.clear();
        assert!(state(&ctx).picks.is_empty());
    }

    #[test]
    fn running_the_add_command_from_a_menu_picks_the_selection() {
        let ctx = egui::Context::default();
        let mut a = DarkBurn::new(Prefs::default());
        let c1 = run(&mut a, "shape.add_ellipse", json!({ "x": 0, "y": 0, "width": 20, "height": 20 })).unwrap();
        let c2 = run(&mut a, "shape.add_ellipse", json!({ "x": 40, "y": 0, "width": 20, "height": 20 })).unwrap();
        run(&mut a, "selection.set", json!({ "ids": [c1["id"], c2["id"]] })).unwrap();
        assert!(run_ui(&ctx, &mut a, ADD));
        assert_eq!(state(&ctx).picks.len(), 2);
        assert!(!state(&ctx).candidates().unwrap().is_empty());
    }

    fn run_ui(ctx: &egui::Context, app: &mut DarkBurn, id: &str) -> bool {
        super::run(app, ctx, id)
    }
}
