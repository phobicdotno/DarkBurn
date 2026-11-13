//! Quick nest and true nest of the selected shapes [ARRAY-10]. The packing itself is
//! `darkburn_geom::sheet_nest`; this file turns the selection into parts and obstacles, runs it
//! and moves the shapes in one undo step. The dialog (`nest_ui.rs`) runs the same steps on a
//! worker thread.
//!
//! What moves and what stays:
//! - Every selected item (a group is one item) moves as one piece, unless it is locked or none
//!   of its shapes is on a layer with output on.
//! - A selected item that lies inside the closed outline of another selected item (a hole, an
//!   engraving on a part) moves with that item.
//! - Locked shapes and shapes on layers with output off stay put and are kept clear of. With
//!   `keep_clear` (on by default) every other shape on the sheet is kept clear of as well.
//! - Shapes on Line layers with output on cut the material: their closed contours can be holes
//!   that other parts go into. Everything else (fills, images, layers with output off) only
//!   takes room.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use darkburn_core::arrange;
use darkburn_core::kurbo::{Affine, BezPath, Point, Rect};
use darkburn_core::{Document, Editor, Mode, Shape, ShapeId};
use darkburn_geom::path::flatten_to_polylines;
use darkburn_geom::sheet_nest::{self, NestPart, NestProgress, SheetNestOptions, SheetNestResult};
use serde_json::{Value, json};

use crate::shape_ops::{flag, in_draw_order, num};
use crate::shell::DarkBurn;

pub const QUICK: &str = "arrange.quick_nest";
pub const TRUE: &str = "arrange.true_nest";

/// Flattening tolerance for the inside tests, mm.
const TOL: f64 = 0.05;
/// Most points of an item tested against an outline it may lie inside.
const PROBES: usize = 200;

pub fn works(id: &str) -> bool {
    id == QUICK || id == TRUE
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    if works(id) { &["ARRAY-10"] } else { &[] }
}

pub fn args_schema(id: &str) -> Option<&'static str> {
    Some(match id {
        QUICK => {
            r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"width":{"type":"number"},"height":{"type":"number"},"gap":{"type":"number","description":"mm kept between parts (default 2)"},"margin":{"type":"number","description":"mm kept clear inside the sheet edge"},"rotation_step":{"type":"number","description":"degrees; 0 for none, 90 for quarter turns (default)"},"max_sheets":{"type":"integer","description":"more sheets of the same size side by side in +x when one is not enough (default 1)"},"sheet_spacing":{"type":"number","description":"mm between sheets (default 10)"},"keep_clear":{"type":"boolean","description":"keep clear of every other shape on the sheet (default true); locked shapes and shapes on layers with output off always are"}},"description":"packs the selected shapes by their bounding boxes; the sheet is the work area when x, y, width, height are missing"}"#
        }
        TRUE => {
            r#"{"type":"object","properties":{"x":{"type":"number"},"y":{"type":"number"},"width":{"type":"number"},"height":{"type":"number"},"gap":{"type":"number","description":"mm kept between parts (default 2)"},"margin":{"type":"number","description":"mm kept clear inside the sheet edge"},"rotation_step":{"type":"number","description":"degrees; 0 for none, 90 for quarter turns (default)"},"allow_holes":{"type":"boolean","description":"small parts may go into the holes of bigger ones (default true)"},"time_limit":{"type":"number","description":"seconds; the best layout found by then is used (default 10)"},"max_sheets":{"type":"integer","description":"more sheets of the same size side by side in +x when one is not enough (default 1)"},"sheet_spacing":{"type":"number","description":"mm between sheets (default 10)"},"keep_clear":{"type":"boolean","description":"keep clear of every other shape on the sheet (default true); locked shapes and shapes on layers with output off always are"}},"description":"packs the selected shapes by their real outlines; the sheet is the work area when x, y, width, height are missing"}"#
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------- settings

/// The rotations for a step in degrees: none for 0, every multiple below a full turn otherwise.
pub fn rotations(step: f64) -> Result<Vec<f64>, String> {
    if !step.is_finite() || step < 0.0 {
        return Err("rotation_step must be 0 or more degrees".into());
    }
    if step < 1e-6 || step >= 360.0 {
        return Ok(vec![0.0]);
    }
    let n = ((360.0 - 1e-9) / step).floor() as usize + 1;
    Ok((0..n.min(3600)).map(|k| k as f64 * step).filter(|a| *a < 360.0 - 1e-9).collect())
}

/// Everything a run needs besides the shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub quick: bool,
    pub sheet: Rect,
    pub opts: SheetNestOptions,
    pub keep_clear: bool,
}

/// The settings from command arguments; `page` is the work area.
pub fn settings(id: &str, a: &Value, page: Rect) -> Result<Settings, String> {
    let sheet = match (a.get("width"), a.get("height")) {
        (Some(_), Some(_)) => {
            let (x, y) = (num(a, "x", page.x0), num(a, "y", page.y0));
            Rect::new(x, y, x + num(a, "width", page.width()), y + num(a, "height", page.height()))
        }
        _ => page,
    };
    let limit = num(a, "time_limit", 10.0);
    if !(limit > 0.0 && limit <= sheet_nest::MAX_TIME_LIMIT) {
        return Err(format!("time_limit must be more than 0 and at most {} seconds", sheet_nest::MAX_TIME_LIMIT));
    }
    let sheets = num(a, "max_sheets", 1.0);
    if !(1.0..=sheet_nest::MAX_SHEETS as f64).contains(&sheets) {
        return Err(format!("max_sheets must be 1 to {}", sheet_nest::MAX_SHEETS));
    }
    let quick = id == QUICK;
    let opts = SheetNestOptions {
        gap: num(a, "gap", 2.0),
        margin: num(a, "margin", 0.0),
        rotations: rotations(num(a, "rotation_step", 90.0))?,
        allow_holes: !quick && flag(a, "allow_holes", true),
        max_sheets: sheets.round() as usize,
        sheet_spacing: num(a, "sheet_spacing", 10.0),
        time_limit: Duration::from_secs_f64(limit),
        tolerance: TOL,
    };
    Ok(Settings { quick, sheet, opts, keep_clear: flag(a, "keep_clear", true) })
}

// ---------------------------------------------------------------- what moves

/// One run: the parts with the shapes each moves, the obstacles, and the settings.
#[derive(Clone, Debug)]
pub struct Job {
    pub parts: Vec<NestPart>,
    pub obstacles: Vec<NestPart>,
    /// The top-level shapes each part moves, in draw order.
    pub members: Vec<Vec<ShapeId>>,
    pub settings: Settings,
    /// The document revision the parts were taken from.
    pub revision: u64,
}

fn on_output(doc: &Document, s: &Shape) -> bool {
    s.leaves().iter().any(|l| doc.layer(l.layer).outputs())
}

/// Whether the leaf cuts through the material: a Line layer with output on.
fn cuts(doc: &Document, leaf: &Shape) -> bool {
    let layer = doc.layer(leaf.layer);
    !leaf.is_image() && layer.outputs() && layer.settings.iter().any(|c| c.enabled && c.mode == Mode::Line)
}

fn part_of(doc: &Document, ids: &[ShapeId]) -> NestPart {
    let mut part = NestPart::default();
    for s in ids.iter().filter_map(|i| doc.get(*i)) {
        for leaf in s.leaves() {
            let path = leaf.outline();
            if path.elements().is_empty() {
                continue;
            }
            if cuts(doc, &leaf) { part.cuts.push(path) } else { part.marks.push(path) }
        }
    }
    part
}

fn inside_ring(ring: &[Point], p: Point) -> bool {
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Up to [`PROBES`] points spread over the outline.
fn probes(path: &BezPath) -> Vec<Point> {
    let pts: Vec<Point> = flatten_to_polylines(path, TOL).into_iter().flat_map(|p| p.points).collect();
    if pts.len() <= PROBES {
        return pts;
    }
    (0..PROBES).map(|i| pts[i * (pts.len() - 1) / (PROBES - 1)]).collect()
}

/// For every item, the smallest other item whose closed outline holds all of it.
fn holders(items: &[(ShapeId, BezPath, Rect)]) -> Vec<Option<usize>> {
    let rings: Vec<Vec<Vec<Point>>> = items
        .iter()
        .map(|(_, p, _)| flatten_to_polylines(p, TOL).into_iter().filter(|l| l.closed && l.points.len() >= 3).map(|l| l.points).collect())
        .collect();
    let probes: Vec<Vec<Point>> = items.iter().map(|(_, p, _)| probes(p)).collect();
    (0..items.len())
        .map(|i| {
            let b = items[i].2;
            let mut best: Option<(f64, usize)> = None;
            for (j, (_, _, ob)) in items.iter().enumerate() {
                let bigger = ob.area() > b.area() + 1e-9;
                let around = ob.x0 <= b.x0 + TOL && ob.y0 <= b.y0 + TOL && ob.x1 >= b.x1 - TOL && ob.y1 >= b.y1 - TOL;
                if j == i || !bigger || !around || rings[j].is_empty() || probes[i].is_empty() {
                    continue;
                }
                // Inside when every point of it lies inside one of the holder's closed contours.
                let held = probes[i].iter().all(|p| rings[j].iter().any(|r| inside_ring(r, *p)));
                if held && best.is_none_or(|(a, _)| ob.area() < a) {
                    best = Some((ob.area(), j));
                }
            }
            best.map(|b| b.1)
        })
        .collect()
}

/// The parts, obstacles and members for nesting `selected` on `doc`.
pub fn collect(doc: &Document, selected: &[ShapeId], settings: Settings, revision: u64) -> Result<Job, String> {
    let chosen = in_draw_order(doc, selected);
    let moving: Vec<ShapeId> = chosen.iter().copied().filter(|i| doc.get(*i).is_some_and(|s| on_output(doc, s))).collect();
    if moving.is_empty() {
        return Err(if chosen.is_empty() { "nothing is selected".into() } else { "the selected shapes are on layers with output off".into() });
    }
    let items: Vec<(ShapeId, BezPath, Rect)> =
        moving.iter().filter_map(|i| doc.get(*i).map(|s| (*i, s.outline(), s.bbox()))).filter(|(_, p, _)| !p.elements().is_empty()).collect();
    let holder = holders(&items);
    let root = |mut i: usize| {
        // Holders are strictly bigger, so this ends.
        while let Some(j) = holder[i] {
            i = j;
        }
        i
    };
    let mut members: Vec<Vec<ShapeId>> = Vec::new();
    let mut slot: Vec<Option<usize>> = vec![None; items.len()];
    for i in 0..items.len() {
        let r = root(i);
        let k = *slot[r].get_or_insert_with(|| {
            members.push(Vec::new());
            members.len() - 1
        });
        members[k].push(items[i].0);
    }
    // Members in draw order (items already are, but a holder can come after what it holds).
    let order: Vec<ShapeId> = doc.iter().map(|s| s.id).collect();
    for m in &mut members {
        m.sort_by_key(|id| order.iter().position(|o| o == id));
    }
    let moved: HashSet<ShapeId> = members.iter().flatten().copied().collect();
    let parts: Vec<NestPart> = members.iter().map(|m| part_of(doc, m)).collect();
    let reach = settings.sheet.inflate(settings.opts.gap.abs() + 1.0, settings.opts.gap.abs() + 1.0);
    let obstacles: Vec<NestPart> = doc
        .iter()
        .filter(|s| !moved.contains(&s.id))
        .filter(|s| settings.keep_clear || s.locked || !on_output(doc, s))
        .filter(|s| s.bbox().overlaps(reach))
        .map(|s| part_of(doc, &[s.id]))
        .filter(|p| !p.cuts.is_empty() || !p.marks.is_empty())
        .collect();
    Ok(Job { parts, obstacles, members, settings, revision })
}

// ---------------------------------------------------------------- running

/// Run the packer. `progress` is called now and then; returning false stops the run, which then
/// returns the best layout found so far.
pub fn pack(job: &Job, progress: &mut dyn FnMut(NestProgress) -> bool) -> Result<SheetNestResult, String> {
    let s = &job.settings;
    let r = if s.quick {
        sheet_nest::quick_nest(&job.parts, &job.obstacles, s.sheet, &s.opts, progress)
    } else {
        sheet_nest::true_nest(&job.parts, &job.obstacles, s.sheet, &s.opts, progress)
    };
    r.map_err(|e| e.to_string())
}

/// Move the shapes as `result` says, in one undo step. Unplaced parts stay where they are.
pub fn apply(e: &mut Editor, job: &Job, result: &SheetNestResult) -> Result<Value, String> {
    if e.revision() != job.revision {
        return Err("the design changed while nesting; nothing was moved".into());
    }
    let label = if job.settings.quick { "Quick Nest" } else { "True Nest" };
    let changed = e.edit(label, |d, _| {
        let mut any = false;
        for (ids, p) in job.members.iter().zip(&result.placements) {
            if let Some(t) = p
                && *t != Affine::IDENTITY
            {
                any |= arrange::transform(d, ids, *t);
            }
        }
        any
    });
    Ok(json!({
        "changed": changed,
        "parts": job.parts.len(),
        "placed": result.placements.iter().flatten().count(),
        "unplaced": result.unplaced,
        "sheets_used": result.sheets_used,
        "utilization": result.utilization,
        "stopped": result.stopped,
        "obstacles": job.obstacles.len(),
        "groups": job.members.iter().map(|m| m.iter().map(|i| i.0).collect::<Vec<_>>()).collect::<Vec<_>>(),
    }))
}

/// The work area.
pub fn page(app: &DarkBurn) -> Rect {
    Rect::new(0.0, 0.0, app.prefs.page_width as f64, app.prefs.page_height as f64)
}

/// Run `id` with JSON arguments, start to end on this thread (agents, scripts, tests).
pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    if !works(id) {
        return None;
    }
    let null = Value::Object(Default::default());
    let a = if a.is_null() { &null } else { a };
    Some((|| {
        let s = settings(id, a, page(app))?;
        let job = collect(&app.editor.doc, &app.editor.selected(), s, app.editor.revision())?;
        let result = pack(&job, &mut |_| true)?;
        apply(&mut app.editor, &job, &result)
    })())
}

/// A run on a worker thread, so the dialog can show progress and stop it.
pub struct NestJob {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    handle: Option<JoinHandle<Result<SheetNestResult, String>>>,
    job: Job,
}

impl NestJob {
    pub fn start(job: Job) -> NestJob {
        let cancel = Arc::new(AtomicBool::new(false));
        let (done, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (c, d, t) = (cancel.clone(), done.clone(), total.clone());
        let work = job.clone();
        let handle = std::thread::spawn(move || {
            pack(&work, &mut |p| {
                d.store(p.done, Ordering::Relaxed);
                t.store(p.total, Ordering::Relaxed);
                !c.load(Ordering::Relaxed)
            })
        });
        NestJob { cancel, done, total, handle: Some(handle), job }
    }

    /// Ask the run to stop; it ends soon with the best layout found so far.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// (done, total) placement steps.
    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub fn finished(&self) -> bool {
        self.handle.as_ref().is_none_or(|h| h.is_finished())
    }

    /// Wait for the end: the job and what the run found.
    pub fn take(mut self) -> (Job, Result<SheetNestResult, String>) {
        let result = match self.handle.take().map(|h| h.join()) {
            Some(Ok(r)) => r,
            _ => Err("the nesting run failed".into()),
        };
        (self.job, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_ops::tests::{add_rect, app, run, select};

    fn bbox(app: &DarkBurn, id: ShapeId) -> Rect {
        app.editor.doc.get(id).unwrap().bbox()
    }

    fn in_sheet(b: Rect, sheet: Rect) -> bool {
        b.x0 >= sheet.x0 - 1e-6 && b.y0 >= sheet.y0 - 1e-6 && b.x1 <= sheet.x1 + 1e-6 && b.y1 <= sheet.y1 + 1e-6
    }

    fn apart(a: Rect, b: Rect, gap: f64) -> bool {
        a.x1 + gap <= b.x0 + 0.06 || b.x1 + gap <= a.x0 + 0.06 || a.y1 + gap <= b.y0 + 0.06 || b.y1 + gap <= a.y0 + 0.06
    }

    #[test]
    fn rotation_steps() {
        assert_eq!(rotations(0.0).unwrap(), vec![0.0]);
        assert_eq!(rotations(90.0).unwrap(), vec![0.0, 90.0, 180.0, 270.0]);
        assert_eq!(rotations(120.0).unwrap(), vec![0.0, 120.0, 240.0]);
        assert_eq!(rotations(400.0).unwrap(), vec![0.0]);
        assert_eq!(rotations(1e-3).unwrap().len(), 3600);
        assert!(rotations(-5.0).is_err());
    }

    #[test]
    fn both_methods_pack_the_selection_into_the_sheet_in_one_undo_step() {
        for id in [QUICK, TRUE] {
            let mut app = app();
            let ids: Vec<ShapeId> = (0..6).map(|i| add_rect(&mut app, 500.0 + i as f64 * 3.0, 500.0, 40.0, 20.0, 0)).collect();
            select(&mut app, &ids);
            let before: Vec<Rect> = ids.iter().map(|i| bbox(&app, *i)).collect();
            let out = run(&mut app, id, json!({ "x": 0, "y": 0, "width": 100, "height": 100, "gap": 2, "time_limit": 2 })).unwrap();
            assert_eq!(out["unplaced"], 0, "{id}: {out}");
            assert_eq!(out["changed"], true);
            let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
            let after: Vec<Rect> = ids.iter().map(|i| bbox(&app, *i)).collect();
            for (i, a) in after.iter().enumerate() {
                assert!(in_sheet(*a, sheet), "{id}: {a:?}");
                for b in &after[i + 1..] {
                    assert!(apart(*a, *b, 2.0), "{id}: {a:?} {b:?}");
                }
            }
            run(&mut app, "edit.undo", Value::Null).unwrap();
            let undone: Vec<Rect> = ids.iter().map(|i| bbox(&app, *i)).collect();
            assert_eq!(undone, before, "{id}: one undo puts every shape back");
        }
    }

    #[test]
    fn groups_move_as_one_and_inner_shapes_stay_with_their_outer_shape() {
        let mut app = app();
        // A frame with an engraving inside it (a separate selected shape), and a group of two.
        let outer = add_rect(&mut app, 300.0, 300.0, 50.0, 50.0, 0);
        let inner = add_rect(&mut app, 310.0, 310.0, 10.0, 10.0, 1);
        let a = add_rect(&mut app, 400.0, 300.0, 10.0, 10.0, 0);
        let b = add_rect(&mut app, 420.0, 300.0, 10.0, 10.0, 0);
        select(&mut app, &[a, b]);
        run(&mut app, "arrange.group", Value::Null).unwrap();
        let gid = app.editor.selected()[0];
        select(&mut app, &[outer, inner, gid]);
        let sizes = |r: Rect| {
            let mut s = [r.width(), r.height()];
            s.sort_by(f64::total_cmp);
            s
        };
        let group_size = sizes(bbox(&app, gid));
        let out = run(&mut app, TRUE, json!({ "x": 0, "y": 0, "width": 200, "height": 200, "time_limit": 2 })).unwrap();
        assert_eq!(out["parts"], 2, "the inner shape belongs to its outer shape: {out}");
        assert_eq!(out["unplaced"], 0, "{out}");
        // Quarter turns may turn the pair; the inner shape still sits where it was in the outer one.
        let (o, i) = (bbox(&app, outer), bbox(&app, inner));
        assert!(in_sheet(i, o), "inner {i:?} left outer {o:?}");
        let d = [i.x0 - o.x0, i.y0 - o.y0, o.x1 - i.x1, o.y1 - i.y1];
        assert!(d.iter().filter(|v| (**v - 10.0).abs() < 1e-6).count() >= 2, "the inner shape moved inside the outer one: {d:?}");
        let gs = sizes(bbox(&app, gid));
        assert!((gs[0] - group_size[0]).abs() < 1e-6 && (gs[1] - group_size[1]).abs() < 1e-6, "the group kept its shape");
        assert_eq!(app.editor.doc.get(gid).map(Shape::is_group), Some(true), "the group stays a group");
    }

    #[test]
    fn locked_and_output_off_shapes_stay_put_and_are_kept_clear_of() {
        let mut app = app();
        let locked = add_rect(&mut app, 0.0, 0.0, 60.0, 60.0, 0);
        let off = add_rect(&mut app, 70.0, 0.0, 30.0, 30.0, 5);
        app.editor.doc.get_mut(locked).unwrap().locked = true;
        app.editor.doc.layer_mut(5).output = false;
        let parts: Vec<ShapeId> = (0..4).map(|i| add_rect(&mut app, 300.0 + 20.0 * i as f64, 300.0, 15.0, 15.0, 0)).collect();
        // Selecting the obstacles too changes nothing for them.
        let mut sel = parts.clone();
        sel.extend([locked, off]);
        select(&mut app, &sel);
        let (lb, ob) = (bbox(&app, locked), bbox(&app, off));
        for id in [QUICK, TRUE] {
            let out = run(&mut app, id, json!({ "x": 0, "y": 0, "width": 100, "height": 100, "gap": 1, "time_limit": 2 })).unwrap();
            assert_eq!(out["unplaced"], 0, "{id}: {out}");
            assert_eq!(bbox(&app, locked), lb);
            assert_eq!(bbox(&app, off), ob);
            for p in &parts {
                let b = bbox(&app, *p);
                assert!(apart(b, lb, 1.0) && apart(b, ob, 1.0), "{id}: {b:?} hits an obstacle");
            }
        }
    }

    #[test]
    fn other_shapes_are_kept_clear_of_unless_asked_not_to() {
        let mut app = app();
        let other = add_rect(&mut app, 0.0, 0.0, 50.0, 50.0, 0);
        let p = add_rect(&mut app, 300.0, 300.0, 20.0, 20.0, 0);
        select(&mut app, &[p]);
        run(&mut app, QUICK, json!({ "x": 0, "y": 0, "width": 100, "height": 100, "gap": 0 })).unwrap();
        assert!(apart(bbox(&app, p), bbox(&app, other), 0.0), "kept clear by default");
        run(&mut app, QUICK, json!({ "x": 0, "y": 0, "width": 100, "height": 100, "gap": 0, "keep_clear": false })).unwrap();
        let b = bbox(&app, p);
        assert!(b.x0.abs() < 1e-6 && b.y0.abs() < 1e-6, "takes the corner when asked: {b:?}");
    }

    #[test]
    fn true_nest_puts_a_small_part_inside_a_cut_hole() {
        let mut app = app();
        // A 60 mm square with a 40 mm square hole (one path), on a sheet only just big enough.
        let frame = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 0 L60 0 L60 60 L0 60 Z M10 10 L10 50 L50 50 L50 10 Z" })).unwrap();
        let frame = ShapeId(frame["id"].as_u64().unwrap());
        let small = add_rect(&mut app, 200.0, 200.0, 30.0, 30.0, 0);
        select(&mut app, &[frame, small]);
        let out = run(&mut app, TRUE, json!({ "x": 0, "y": 0, "width": 62, "height": 62, "gap": 1, "time_limit": 3 })).unwrap();
        assert_eq!(out["unplaced"], 0, "{out}");
        let (f, s) = (bbox(&app, frame), bbox(&app, small));
        assert!(in_sheet(s, f), "the small part went into the hole: {s:?} in {f:?}");
        // Holes off: it no longer fits.
        run(&mut app, "edit.undo", Value::Null).unwrap();
        let out = run(&mut app, TRUE, json!({ "x": 0, "y": 0, "width": 62, "height": 62, "gap": 1, "time_limit": 3, "allow_holes": false })).unwrap();
        assert_eq!(out["unplaced"], 1, "{out}");
    }

    #[test]
    fn several_sheets_side_by_side() {
        let mut app = app();
        let ids: Vec<ShapeId> = (0..4).map(|i| add_rect(&mut app, 300.0 + 50.0 * i as f64, 300.0, 40.0, 40.0, 0)).collect();
        select(&mut app, &ids);
        let out = run(&mut app, QUICK, json!({ "x": 0, "y": 0, "width": 50, "height": 50, "max_sheets": 4, "sheet_spacing": 10 })).unwrap();
        assert_eq!(out["unplaced"], 0, "{out}");
        assert_eq!(out["sheets_used"], 4, "{out}");
        let mut xs: Vec<f64> = ids.iter().map(|i| bbox(&app, *i).x0).collect();
        xs.sort_by(f64::total_cmp);
        for (k, x) in xs.iter().enumerate() {
            assert!((x - 60.0 * k as f64).abs() < 1e-6, "sheet {k} starts at {}: {x}", 60.0 * k as f64);
        }
    }

    #[test]
    fn bad_arguments_are_refused_and_change_nothing() {
        let mut app = app();
        let r = add_rect(&mut app, 300.0, 300.0, 20.0, 20.0, 0);
        select(&mut app, &[]);
        assert!(run(&mut app, TRUE, json!({})).is_err(), "nothing selected");
        select(&mut app, &[r]);
        let rev = app.editor.revision();
        for a in [
            json!({ "time_limit": 0 }),
            json!({ "time_limit": 1e9 }),
            json!({ "max_sheets": 0 }),
            json!({ "max_sheets": 1000 }),
            json!({ "rotation_step": -1 }),
            json!({ "gap": -1 }),
            json!({ "x": 0, "y": 0, "width": 0, "height": 10 }),
            json!({ "x": 0, "y": 0, "width": -5, "height": 10 }),
            json!({ "x": 1e300, "y": 0, "width": 5, "height": 10 }),
        ] {
            assert!(run(&mut app, QUICK, a.clone()).is_err() || run(&mut app, TRUE, a.clone()).is_err(), "{a}");
            assert_eq!(app.editor.revision(), rev, "{a} changed the design");
        }
        // A sheet too small for the part: nothing moves, nothing breaks.
        let out = run(&mut app, TRUE, json!({ "x": 0, "y": 0, "width": 5, "height": 5, "time_limit": 1 })).unwrap();
        assert_eq!(out["unplaced"], 1);
        assert_eq!(out["changed"], false);
    }

    #[test]
    fn a_worker_run_that_is_cancelled_keeps_what_it_found() {
        let mut app = app();
        let ids: Vec<ShapeId> = (0..40).map(|i| add_rect(&mut app, 300.0 + 12.0 * i as f64, 300.0, 10.0, 7.0, 0)).collect();
        select(&mut app, &ids);
        let s = settings(TRUE, &json!({ "x": 0, "y": 0, "width": 200, "height": 200, "time_limit": 30 }), page(&app)).unwrap();
        let job = collect(&app.editor.doc, &app.editor.selected(), s, app.editor.revision()).unwrap();
        let worker = NestJob::start(job);
        worker.cancel();
        assert!(worker.cancelled());
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while !worker.finished() {
            assert!(std::time::Instant::now() < end, "the cancelled run never ended");
            std::thread::sleep(Duration::from_millis(2));
        }
        let (job, result) = worker.take();
        let result = result.unwrap();
        assert!(result.stopped);
        let out = apply(&mut app.editor, &job, &result).unwrap();
        assert_eq!(out["stopped"], true);
        // A design changed in the meantime is not touched.
        let other = add_rect(&mut app, 0.0, 300.0, 5.0, 5.0, 0);
        let _ = other;
        assert!(apply(&mut app.editor, &job, &result).is_err());
    }

    #[test]
    fn registered_listed_and_on_the_checklist() {
        for id in [QUICK, TRUE] {
            assert!(crate::commands::works(id), "{id}");
            assert_eq!(crate::commands::checklist(id), &["ARRAY-10"]);
            assert!(crate::commands::registry().iter().any(|c| c.id == id && c.args.contains("gap")), "{id}");
        }
    }
}
