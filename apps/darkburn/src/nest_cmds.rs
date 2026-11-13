//! Quick nest and true nest of the selected shapes [ARRAY-10]. The packing itself is
//! `darkburn_geom::sheet_nest`; this file turns the selection into parts and obstacles, runs it
//! and moves the shapes in one undo step. The dialog (`nest_ui.rs`) runs the same steps on a
//! worker thread.
//!
//! What moves and what stays:
//! - Every selected item (a group is one item) moves as one piece, unless it is locked or none
//!   of its shapes is on a layer with output on.
//! - A selected item that lies on another selected item (an engraving on a part, a hole cut
//!   as a shape of its own) moves with that item. It lies on the item when every point of it is
//!   inside the item's material: inside an odd number of its closed cut outlines (so a part
//!   sitting in a hole is not on the frame around it), or inside its fills or images.
//! - Locked shapes and shapes on layers with output off stay put and are kept clear of. With
//!   `keep_clear` (on by default) every other shape on the sheet is kept clear of as well.
//! - Shapes on Line layers with output on cut the material: their closed contours can be holes
//!   that other parts go into. Everything else (fills, images, layers with output off) only
//!   takes room. A shape on a layer with output off whose box covers a whole sheet is a drawn
//!   material outline: it counts by its line, so the parts go inside it.
//! - The selection is sorted out on the worker thread too, and that part also stops on Cancel
//!   and at the time limit (then nothing moves). A finished run moves the shapes only when the
//!   design is still the one it was started on.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use darkburn_core::arrange;
use darkburn_core::kurbo::{Affine, Point, Rect};
use darkburn_core::{Document, Editor, Mode, Shape, ShapeId};
use darkburn_geom::path::flatten_to_polylines;
use darkburn_geom::sheet_nest::{self, NestPart, NestProgress, SheetNestOptions, SheetNestResult};
use serde_json::{Value, json};

use crate::shape_ops::{flag, num};
use crate::shell::DarkBurn;

pub const QUICK: &str = "arrange.quick_nest";
pub const TRUE: &str = "arrange.true_nest";

/// Flattening tolerance for the inside tests, mm.
const TOL: f64 = 0.05;
/// Most points of an item tested against an outline it may lie inside.
const PROBES: usize = 200;
/// Most bigger items an item is tested against to find the one it lies on, smallest first.
const CANDIDATES: usize = 16;
/// Why a run that was stopped before it started placing moved nothing.
const STOPPED: &str = "the run was stopped before any shape was placed; nothing was moved";

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

/// Most rotations handed to the packer (which thins them further, evenly).
const MAX_STEPS: usize = 3600;

/// The rotations for a step in degrees: none for 0, every multiple below a full turn otherwise.
/// A step so fine that there would be more than [`MAX_STEPS`] keeps that many multiples, spread
/// evenly over the whole turn.
pub fn rotations(step: f64) -> Result<Vec<f64>, String> {
    if !step.is_finite() || step < 0.0 {
        return Err("rotation_step must be 0 or more degrees".into());
    }
    if step < 1e-6 || step >= 360.0 {
        return Ok(vec![0.0]);
    }
    let n = ((360.0 - 1e-9) / step).floor() as usize + 1;
    let k = |i: usize| if n > MAX_STEPS { i * n / MAX_STEPS } else { i };
    Ok((0..n.min(MAX_STEPS)).map(|i| k(i) as f64 * step).filter(|a| *a < 360.0 - 1e-9).collect())
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
            let (w, h) = (num(a, "width", page.width()), num(a, "height", page.height()));
            if !(w > 0.0 && h > 0.0) {
                return Err("the sheet's width and height must be more than 0".into());
            }
            Rect::new(x, y, x + w, y + h)
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
    /// The design the parts were taken from (cheap: shapes are shared).
    pub doc: Document,
}

fn on_output(doc: &Document, s: &Shape) -> bool {
    s.leaves().iter().any(|l| doc.layer(l.layer).outputs())
}

/// Whether the leaf cuts through the material: a Line layer with output on.
fn cuts(doc: &Document, leaf: &Shape) -> bool {
    let layer = doc.layer(leaf.layer);
    !leaf.is_image() && layer.outputs() && layer.settings.iter().any(|c| c.enabled && c.mode == Mode::Line)
}

fn covers(outer: &Rect, inner: &Rect) -> bool {
    outer.x0 <= inner.x0 + TOL && outer.y0 <= inner.y0 + TOL && outer.x1 >= inner.x1 - TOL && outer.y1 >= inner.y1 - TOL
}

/// The shapes `ids` as one part. A leaf on a layer with output off whose box covers one of
/// `frames` (the sheets' usable areas) is a drawn material outline: it never burns, so it is
/// handed over as a cut, which counts by its line when it surrounds a sheet.
fn part_of(doc: &Document, ids: &[ShapeId], frames: &[Rect]) -> NestPart {
    let mut part = NestPart::default();
    for s in ids.iter().filter_map(|i| doc.get(*i)) {
        for leaf in s.leaves() {
            let path = leaf.outline();
            if path.elements().is_empty() {
                continue;
            }
            let outline = !doc.layer(leaf.layer).outputs() && frames.iter().any(|f| covers(&leaf.bbox(), f));
            if outline || cuts(doc, &leaf) { part.cuts.push(path) } else { part.marks.push(path) }
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

/// A closed ring with its box.
type Ring = (Rect, Vec<Point>);

/// A selected item, for finding what it lies on.
struct Item {
    id: ShapeId,
    bbox: Rect,
    /// Closed rings of its cutting leaves, and of everything else (fills, images, engraving).
    cut: Vec<Ring>,
    mark: Vec<Ring>,
    /// Up to [`PROBES`] points spread over its outline.
    probes: Vec<Point>,
}

fn item(doc: &Document, s: &Shape) -> Item {
    let (mut cut, mut mark, mut all) = (Vec::new(), Vec::new(), Vec::new());
    for leaf in s.leaves() {
        let is_cut = cuts(doc, &leaf);
        for pl in flatten_to_polylines(&leaf.outline(), TOL) {
            all.extend_from_slice(&pl.points);
            if pl.closed && pl.points.len() >= 3 {
                let b = pl.points.iter().fold(Rect::from_points(pl.points[0], pl.points[0]), |r, p| r.union_pt(*p));
                if is_cut { cut.push((b, pl.points)) } else { mark.push((b, pl.points)) }
            }
        }
    }
    let probes = if all.len() <= PROBES { all } else { (0..PROBES).map(|i| all[i * (all.len() - 1) / (PROBES - 1)]).collect() };
    Item { id: s.id, bbox: s.bbox(), cut, mark, probes }
}

/// The closed cut (or other) rings of the items `on` whose box holds `bbox`.
fn rings_over<'a>(items: &'a [Item], on: &[usize], cut: bool, bbox: &Rect) -> Vec<&'a [Point]> {
    let rings = on.iter().flat_map(|m| if cut { &items[*m].cut } else { &items[*m].mark });
    rings.filter(|(b, _)| covers(b, bbox)).map(|(_, r)| r.as_slice()).collect()
}

/// True when every probe of `it` lies in the material of the items `on`: inside an odd number of
/// their closed cut rings, or of their other closed rings. Rings whose box does not hold the
/// whole item are left out.
fn held(items: &[Item], on: &[usize], it: &Item) -> bool {
    let (cut, mark) = (rings_over(items, on, true, &it.bbox), rings_over(items, on, false, &it.bbox));
    if it.probes.is_empty() || (cut.is_empty() && mark.is_empty()) {
        return false;
    }
    let odd = |rings: &[&[Point]], p: Point| rings.iter().filter(|r| inside_ring(r, p)).count() % 2 == 1;
    it.probes.iter().all(|p| odd(&cut, *p) || odd(&mark, *p))
}

/// Sorts the items into parts: biggest first, every item joins the smallest bigger part it lies
/// on (trying at most [`CANDIDATES`]), or starts a part of its own. Parts and their members come
/// in draw order. Err when `stop` says so.
fn grouped(items: &[Item], stop: &mut dyn FnMut() -> bool) -> Result<Vec<Vec<usize>>, String> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[b].bbox.area().total_cmp(&items[a].bbox.area()).then(a.cmp(&b)));
    let mut groups: Vec<(Rect, Vec<usize>)> = Vec::new();
    for &i in &order {
        if stop() {
            return Err(STOPPED.into());
        }
        let it = &items[i];
        let mut cands: Vec<usize> =
            (0..groups.len()).filter(|&g| groups[g].0.area() > it.bbox.area() + 1e-9 && covers(&groups[g].0, &it.bbox)).collect();
        cands.sort_by(|&a, &b| groups[a].0.area().total_cmp(&groups[b].0.area()).then(a.cmp(&b)));
        let mut home = None;
        for g in cands.into_iter().take(CANDIDATES) {
            if stop() {
                return Err(STOPPED.into());
            }
            if held(items, &groups[g].1, it) {
                home = Some(g);
                break;
            }
        }
        match home {
            Some(g) => groups[g].1.push(i),
            None => groups.push((it.bbox, vec![i])),
        }
    }
    let mut out: Vec<Vec<usize>> = groups.into_iter().map(|g| g.1).collect();
    for m in &mut out {
        m.sort_unstable();
    }
    out.sort_by_key(|m| m[0]);
    Ok(out)
}

/// The parts, obstacles and members for nesting `selected` on `doc`. `stop` is asked now and
/// then; when it says so, the result is an error and nothing is to be moved.
pub fn collect(doc: &Document, selected: &[ShapeId], settings: Settings, revision: u64, stop: &mut dyn FnMut() -> bool) -> Result<Job, String> {
    let wanted: HashSet<ShapeId> = selected.iter().copied().collect();
    let chosen: Vec<&Arc<Shape>> = doc.shapes.iter().filter(|s| wanted.contains(&s.id) && !s.locked).collect();
    let moving: Vec<&Arc<Shape>> = chosen.iter().copied().filter(|s| on_output(doc, s)).collect();
    if moving.is_empty() {
        return Err(if chosen.is_empty() { "nothing is selected".into() } else { "the selected shapes are on layers with output off".into() });
    }
    let mut items: Vec<Item> = Vec::with_capacity(moving.len());
    for s in moving.iter().filter(|s| !s.outline().elements().is_empty()) {
        if stop() {
            return Err(STOPPED.into());
        }
        items.push(item(doc, s));
    }
    let members: Vec<Vec<ShapeId>> = grouped(&items, stop)?.into_iter().map(|m| m.into_iter().map(|i| items[i].id).collect()).collect();
    let moved: HashSet<ShapeId> = members.iter().flatten().copied().collect();
    let parts: Vec<NestPart> = members.iter().map(|m| part_of(doc, m, &[])).collect();
    // Every sheet the run may use, and the margin's usable area of each.
    let o = &settings.opts;
    let sheets: Vec<Rect> = (0..o.max_sheets.max(1)).map(|k| sheet_nest::sheet_rect(settings.sheet, o.sheet_spacing, k)).collect();
    let frames: Vec<Rect> = sheets.iter().map(|r| r.inset(-o.margin.max(0.0))).collect();
    let g = o.gap.abs() + 1.0;
    let reach: Vec<Rect> = sheets.iter().map(|r| r.inflate(g, g)).collect();
    let mut obstacles: Vec<NestPart> = Vec::new();
    for s in doc.iter() {
        if moved.contains(&s.id) || !(settings.keep_clear || s.locked || !on_output(doc, s)) {
            continue;
        }
        let b = s.bbox();
        if !reach.iter().any(|r| b.overlaps(*r)) {
            continue;
        }
        let p = part_of(doc, &[s.id], &frames);
        if !p.cuts.is_empty() || !p.marks.is_empty() {
            obstacles.push(p);
        }
    }
    Ok(Job { parts, obstacles, members, settings, revision, doc: doc.clone() })
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

/// Sort out the selection and pack it, all within the time limit. `cancel` is asked now and
/// then; `progress` hears how the packing goes.
pub fn prepare_and_pack(
    doc: &Document,
    selected: &[ShapeId],
    settings: Settings,
    revision: u64,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(NestProgress),
) -> Result<(Job, SheetNestResult), String> {
    let t0 = Instant::now();
    let limit = settings.opts.time_limit;
    let mut job = collect(doc, selected, settings, revision, &mut || cancel() || t0.elapsed() >= limit)?;
    // The packing gets what is left of the time.
    job.settings.opts.time_limit = limit.saturating_sub(t0.elapsed());
    let result = pack(&job, &mut |p| {
        progress(p);
        !cancel()
    });
    job.settings.opts.time_limit = limit;
    Ok((job, result?))
}

/// Same design: the shapes and layers the parts were taken from (notes and job settings may
/// differ).
fn same_design(a: &Document, b: &Document) -> bool {
    a.shapes.len() == b.shapes.len()
        && a.shapes.iter().zip(&b.shapes).all(|(x, y)| Arc::ptr_eq(x, y) || x == y)
        && a.layers == b.layers
        && a.layer_order == b.layer_order
}

/// Move the shapes as `result` says, in one undo step. Unplaced parts stay where they are.
/// Nothing moves when the design is not the one the run started on (it was edited, or another
/// project was opened, in the meantime).
pub fn apply(e: &mut Editor, job: &Job, result: &SheetNestResult) -> Result<Value, String> {
    if e.revision() != job.revision || !same_design(&e.doc, &job.doc) {
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
        let (job, result) = prepare_and_pack(&app.editor.doc, &app.editor.selected(), s, app.editor.revision(), &|| false, &mut |_| {})?;
        apply(&mut app.editor, &job, &result)
    })())
}

/// A run on a worker thread, so the dialog can show progress and stop it.
pub struct NestJob {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    handle: Option<JoinHandle<Result<(Job, SheetNestResult), String>>>,
}

impl NestJob {
    /// Start nesting `selected` of `doc` (a copy of the open design).
    pub fn start(doc: Document, selected: Vec<ShapeId>, settings: Settings, revision: u64) -> NestJob {
        let cancel = Arc::new(AtomicBool::new(false));
        let (done, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (c, d, t) = (cancel.clone(), done.clone(), total.clone());
        let handle = std::thread::spawn(move || {
            prepare_and_pack(&doc, &selected, settings, revision, &|| c.load(Ordering::Relaxed), &mut |p| {
                d.store(p.done, Ordering::Relaxed);
                t.store(p.total, Ordering::Relaxed);
            })
        });
        NestJob { cancel, done, total, handle: Some(handle) }
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
    pub fn take(mut self) -> Result<(Job, SheetNestResult), String> {
        match self.handle.take().map(|h| h.join()) {
            Some(Ok(r)) => r,
            _ => Err("the nesting run failed".into()),
        }
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
        // A step finer than a tenth of a degree still covers the whole turn.
        for step in [0.05, 0.01, 1e-3] {
            let r = rotations(step).unwrap();
            let top = r.iter().copied().fold(0.0, f64::max);
            assert!(top > 359.0 && top < 360.0, "{step}: up to {top}");
            assert!(r.windows(2).all(|w| w[1] > w[0]));
        }
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
        let wait = |worker: &NestJob| {
            let end = Instant::now() + Duration::from_secs(20);
            while !worker.finished() {
                assert!(Instant::now() < end, "the cancelled run never ended");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        // Cancelled at once (most likely while the selection is still being sorted out):
        // nothing to move, or a stopped run.
        let worker = NestJob::start(app.editor.doc.clone(), app.editor.selected(), s.clone(), app.editor.revision());
        worker.cancel();
        assert!(worker.cancelled());
        wait(&worker);
        match worker.take() {
            Err(e) => assert_eq!(e, STOPPED),
            Ok((_, r)) => assert!(r.stopped),
        }
        // Cancelled while placing: what it found so far is used.
        let worker = NestJob::start(app.editor.doc.clone(), app.editor.selected(), s, app.editor.revision());
        let end = Instant::now() + Duration::from_secs(20);
        while worker.progress().1 == 0 && !worker.finished() {
            assert!(Instant::now() < end, "the run never started placing");
            std::thread::sleep(Duration::from_millis(1));
        }
        worker.cancel();
        wait(&worker);
        let (job, result) = worker.take().unwrap();
        assert!(result.stopped);
        let out = apply(&mut app.editor, &job, &result).unwrap();
        assert_eq!(out["stopped"], true);
        // A design changed in the meantime is not touched.
        let other = add_rect(&mut app, 0.0, 300.0, 5.0, 5.0, 0);
        let _ = other;
        assert!(apply(&mut app.editor, &job, &result).is_err());
    }

    /// Shapes on a fresh design: (design, the shapes).
    fn design(rects: &[(f64, f64, f64, f64)]) -> (Document, Vec<ShapeId>) {
        let mut app = app();
        let ids = rects.iter().map(|r| add_rect(&mut app, r.0, r.1, r.2, r.3, 0)).collect();
        (app.editor.doc.clone(), ids)
    }

    #[test]
    fn a_run_that_ends_after_another_project_was_opened_moves_nothing() {
        let (doc_a, ids) = design(&[(300.0, 300.0, 20.0, 20.0), (340.0, 300.0, 20.0, 20.0)]);
        // Another project: the same shapes, but somewhere else. Both open unmodified (revision 0).
        let mut doc_b = doc_a.clone();
        arrange::transform(&mut doc_b, &ids, Affine::translate((-150.0, 40.0)));
        let mut app = app();
        app.editor = Editor::with_document(doc_a.clone());
        app.editor.selection.set(ids.clone());
        let s = settings(QUICK, &json!({ "x": 0, "y": 0, "width": 100, "height": 100 }), page(&app)).unwrap();
        let (job, result) = prepare_and_pack(&app.editor.doc, &app.editor.selected(), s, app.editor.revision(), &|| false, &mut |_| {}).unwrap();
        app.editor = Editor::with_document(doc_b.clone());
        assert_eq!(app.editor.revision(), job.revision);
        assert!(apply(&mut app.editor, &job, &result).is_err());
        assert_eq!(app.editor.doc, doc_b, "the other project is untouched");
        // Reopening the same design is fine.
        app.editor = Editor::with_document(doc_a);
        assert_eq!(apply(&mut app.editor, &job, &result).unwrap()["changed"], true);
    }

    #[test]
    fn a_part_sitting_in_a_hole_is_not_welded_to_the_frame_on_the_next_run() {
        let mut app = app();
        let frame = run(&mut app, "shape.add_path", json!({ "svg_path": "M0 0 L60 0 L60 60 L0 60 Z M10 10 L10 50 L50 50 L50 10 Z" })).unwrap();
        let frame = ShapeId(frame["id"].as_u64().unwrap());
        let small = add_rect(&mut app, 200.0, 200.0, 30.0, 30.0, 0);
        select(&mut app, &[frame, small]);
        let sheet = json!({ "x": 0, "y": 0, "width": 62, "height": 62, "gap": 1, "time_limit": 3 });
        let out = run(&mut app, TRUE, sheet.clone()).unwrap();
        assert_eq!(out["unplaced"], 0, "{out}");
        assert!(in_sheet(bbox(&app, small), bbox(&app, frame)));
        // Run again as it lies: the part in the hole is still a part of its own.
        let mut again = sheet;
        again["allow_holes"] = json!(false);
        let out = run(&mut app, TRUE, again).unwrap();
        assert_eq!(out["parts"], 2, "{out}");
        assert_eq!(out["unplaced"], 1, "{out}");
        // An engraving on the frame's material still goes with the frame.
        let mut app = crate::shape_ops::tests::app();
        let frame =
            run(&mut app, "shape.add_path", json!({ "svg_path": "M300 300 L360 300 L360 360 L300 360 Z M310 310 L310 350 L350 350 L350 310 Z" }))
                .unwrap();
        let frame = ShapeId(frame["id"].as_u64().unwrap());
        let mark = add_rect(&mut app, 302.0, 302.0, 5.0, 5.0, 1);
        select(&mut app, &[frame, mark]);
        let out = run(&mut app, TRUE, json!({ "x": 0, "y": 0, "width": 100, "height": 100, "time_limit": 2 })).unwrap();
        assert_eq!(out["parts"], 1, "{out}");
    }

    #[test]
    fn obstacles_on_later_sheets_are_kept_clear_of() {
        for id in [QUICK, TRUE] {
            let mut app = app();
            // Sheet 1 runs from x 110 to 210; this leaves only a 28 mm strip of it.
            let locked = add_rect(&mut app, 110.0, 0.0, 70.0, 100.0, 0);
            app.editor.doc.get_mut(locked).unwrap().locked = true;
            let ids: Vec<ShapeId> = (0..3).map(|i| add_rect(&mut app, 400.0 + 70.0 * i as f64, 300.0, 60.0, 60.0, 0)).collect();
            select(&mut app, &ids);
            let a = json!({ "x": 0, "y": 0, "width": 100, "height": 100, "max_sheets": 3, "sheet_spacing": 10, "gap": 2, "time_limit": 2 });
            let out = run(&mut app, id, a).unwrap();
            assert_eq!(out["unplaced"], 1, "{id}: {out}");
            assert_eq!(out["sheets_used"], 3, "{id}: {out}");
            let lb = bbox(&app, locked);
            for p in &ids {
                let b = bbox(&app, *p);
                assert!(b.x0 >= 400.0 || apart(b, lb, 2.0), "{id}: {b:?} is on the obstacle");
            }
        }
    }

    #[test]
    fn a_material_outline_frames_the_sheet_but_a_fill_over_it_takes_it_all() {
        for id in [QUICK, TRUE] {
            let mut app = app();
            // The material drawn on a layer with output off, just around the sheet.
            add_rect(&mut app, -1.0, -1.0, 102.0, 102.0, 5);
            app.editor.doc.layer_mut(5).output = false;
            let ids: Vec<ShapeId> = (0..4).map(|i| add_rect(&mut app, 300.0 + 30.0 * i as f64, 300.0, 20.0, 20.0, 0)).collect();
            select(&mut app, &ids);
            let a = json!({ "x": 0, "y": 0, "width": 100, "height": 100, "gap": 2, "time_limit": 2 });
            let out = run(&mut app, id, a.clone()).unwrap();
            assert_eq!(out["unplaced"], 0, "{id}: {out}");
            // A filled shape over the whole sheet (it burns all of it): nothing fits.
            run(&mut app, "edit.undo", Value::Null).unwrap();
            add_rect(&mut app, -1.0, -1.0, 102.0, 102.0, 6);
            for c in &mut app.editor.doc.layer_mut(6).settings {
                c.mode = Mode::Fill;
            }
            select(&mut app, &ids);
            let out = run(&mut app, id, a).unwrap();
            assert_eq!(out["unplaced"], 4, "{id}: {out}");
        }
    }

    /// `n` concentric 100-point rings around (1000, 1000), each a selected shape.
    fn concentric(app: &mut DarkBurn, n: usize) -> Vec<ShapeId> {
        (0..n)
            .map(|i| {
                let r = 10.0 + i as f64 * 0.5;
                let pts: Vec<String> = (0..100)
                    .map(|k| {
                        let a = k as f64 * std::f64::consts::TAU / 100.0;
                        format!("{} {}", 1000.0 + r * a.cos(), 1000.0 + r * a.sin())
                    })
                    .collect();
                let v = run(app, "shape.add_path", json!({ "svg_path": format!("M{} Z", pts.join(" L")) })).unwrap();
                ShapeId(v["id"].as_u64().unwrap())
            })
            .collect()
    }

    #[test]
    fn sorting_out_a_big_selection_is_bounded_and_stops() {
        let mut app = app();
        let ids = concentric(&mut app, 1500);
        let s = settings(QUICK, &json!({ "x": 0, "y": 0, "width": 100, "height": 100 }), page(&app)).unwrap();
        let t0 = Instant::now();
        let job = collect(&app.editor.doc, &ids, s.clone(), 0, &mut || false).unwrap();
        assert!(t0.elapsed() < Duration::from_secs(20), "took {:?}", t0.elapsed());
        // Every ring lies on the next bigger one or in its hole, turn about: frames with holes.
        assert_eq!(job.parts.len(), 750);
        assert!(job.members.iter().all(|m| m.len() == 2));
        let t0 = Instant::now();
        assert!(collect(&app.editor.doc, &ids, s.clone(), 0, &mut || true).is_err());
        let mut calls = 0;
        assert!(
            collect(&app.editor.doc, &ids, s, 0, &mut || {
                calls += 1;
                calls > 2000
            })
            .is_err()
        );
        assert!(t0.elapsed() < Duration::from_secs(5), "stopping took {:?}", t0.elapsed());
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
