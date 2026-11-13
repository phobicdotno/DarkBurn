//! Dialogs and ribbon controls of the shape-editing commands (offset, boolean assistant, close
//! with tolerance, auto-join, resize slots, arrays, nest, alignment helper). Every dialog builds
//! the JSON arguments of a registered command (`shape_ops.rs`) and runs it, so the dialog holds
//! no logic of its own; previews call the `*_preview` functions of `shape_ops` and
//! `shape_arrays`. The dialogs are ordinary windows, not modal: zoom and pan keep working.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use darkburn_core::kurbo::{BezPath, Rect};
use darkburn_core::varray::{self, AlongDef, ArrayDef, ArrayKind, CircularDef, GridDef, GridShift};
use darkburn_core::{Editor, ShapeId};
use darkburn_geom::array as garray;
use darkburn_geom::cleanup;
use darkburn_geom::slots::SlotKind;
use darkburn_ui::ui_kit;
use egui::{Color32, Context, Id, Pos2, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::numeric::{self, Num};
use crate::shell::DarkBurn;

/// The commands that open a dialog when run from a menu or button (headless callers pass
/// arguments instead and never see a dialog).
pub const DIALOG_COMMANDS: &[&str] = &[
    "tools.offset",
    "tools.boolean_assistant",
    "tools.resize_slots",
    "edit.close_tolerance",
    "edit.auto_join",
    "arrange.grid_array",
    "arrange.circular_array",
    "arrange.copy_along_path",
    "arrange.nest",
    "arrange.alignment_helper",
    "arrange.array_edit",
    "arrange.move_selection",
];

/// How long a status message stays up, in seconds.
const STATUS_SECONDS: f64 = 6.0;
/// Previews with more copies than this are not drawn.
const PREVIEW_COPIES: usize = 2000;

// ---------------------------------------------------------------- forms

/// Offset Shapes settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetForm {
    pub distance: f64,
    pub side: usize,
    pub join: usize,
    pub delete_original: bool,
    pub select_results: bool,
    pub outer_only: bool,
    pub open_shapes: bool,
}

impl Default for OffsetForm {
    fn default() -> Self {
        OffsetForm { distance: 1.0, side: 0, join: 0, delete_original: false, select_results: true, outer_only: false, open_shapes: false }
    }
}

pub const SIDES: [&str; 3] = ["outward", "inward", "both"];
pub const JOINS: [&str; 3] = ["round", "bevel", "sharp"];

impl OffsetForm {
    pub fn args(&self) -> Value {
        json!({
            "distance": self.distance, "side": SIDES[self.side.min(2)], "join": JOINS[self.join.min(2)],
            "delete_original": self.delete_original, "select_results": self.select_results,
            "outer_only": self.outer_only, "open_shapes": self.open_shapes,
        })
    }
}

/// Boolean assistant settings: the two sets and the operation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoolForm {
    pub a: Vec<ShapeId>,
    pub b: Vec<ShapeId>,
    /// 0 union, 1 A minus B, 2 B minus A, 3 intersection, 4 xor.
    pub op: usize,
    pub keep: bool,
}

pub const BOOL_OPS: [&str; 5] = ["Union", "A minus B", "B minus A", "Intersection", "Xor"];

impl BoolForm {
    pub fn args(&self) -> Value {
        let ids = |v: &[ShapeId]| v.iter().map(|i| i.0).collect::<Vec<_>>();
        let (op, a, b) = match self.op {
            0 => ("union", &self.a, &self.b),
            1 => ("difference", &self.a, &self.b),
            2 => ("difference", &self.b, &self.a),
            3 => ("intersection", &self.a, &self.b),
            _ => ("xor", &self.a, &self.b),
        };
        json!({ "op": op, "a": ids(a), "b": ids(b), "keep_originals": self.keep })
    }
}

/// Resize slots settings; a limit of 0 means no limit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotForm {
    pub min_w: f64,
    pub max_w: f64,
    pub min_d: f64,
    pub max_d: f64,
    pub include_tabs: bool,
    pub set_w: bool,
    pub new_w: f64,
    pub set_d: bool,
    pub new_d: f64,
}

impl SlotForm {
    /// The filter only (used to find and highlight).
    pub fn query_args(&self) -> Value {
        let mut a = json!({ "min_width": self.min_w, "min_depth": self.min_d, "include_tabs": self.include_tabs });
        if self.max_w > 0.0 {
            a["max_width"] = json!(self.max_w);
        }
        if self.max_d > 0.0 {
            a["max_depth"] = json!(self.max_d);
        }
        a
    }

    pub fn args(&self) -> Value {
        let mut a = self.query_args();
        if self.set_w {
            a["new_width"] = json!(self.new_w);
        }
        if self.set_d {
            a["new_depth"] = json!(self.new_d);
        }
        a
    }
}

/// Grid array settings.
#[derive(Clone, Debug, PartialEq)]
pub struct GridForm {
    pub rows: f64,
    pub cols: f64,
    pub fit: bool,
    pub fit_w: f64,
    pub fit_h: f64,
    pub c2c: bool,
    pub dx: f64,
    pub dy: f64,
    /// 0 none, 1 alternate rows, 2 alternate columns, 3 hex.
    pub shift: usize,
    pub shift_amount: f64,
    pub mirror_rows: bool,
    pub mirror_cols: bool,
    pub rev_x: bool,
    pub rev_y: bool,
    pub virtual_array: bool,
}

impl Default for GridForm {
    fn default() -> Self {
        GridForm {
            rows: 2.0,
            cols: 2.0,
            fit: false,
            fit_w: 100.0,
            fit_h: 100.0,
            c2c: false,
            dx: 2.0,
            dy: 2.0,
            shift: 0,
            shift_amount: 0.0,
            mirror_rows: false,
            mirror_cols: false,
            rev_x: false,
            rev_y: false,
            virtual_array: true,
        }
    }
}

pub const SHIFTS: [&str; 4] = ["none", "alt_rows", "alt_cols", "hex"];

impl GridForm {
    pub fn args(&self) -> Value {
        let mut a = json!({
            "rows": self.rows.round().max(1.0), "cols": self.cols.round().max(1.0), "centre_to_centre": self.c2c,
            "dx": self.dx, "dy": self.dy, "shift": SHIFTS[self.shift.min(3)], "shift_amount": self.shift_amount,
            "mirror_alt_rows": self.mirror_rows, "mirror_alt_cols": self.mirror_cols,
            "reverse_x": self.rev_x, "reverse_y": self.rev_y, "virtual": self.virtual_array,
        });
        if self.fit {
            a["fit_width"] = json!(self.fit_w);
            a["fit_height"] = json!(self.fit_h);
        }
        a
    }

    /// The form for an existing grid definition.
    pub fn from_def(g: &GridDef) -> GridForm {
        let (shift, amount) = match g.shift {
            GridShift::None => (0, 0.0),
            GridShift::AltRows(v) => (1, v),
            GridShift::AltCols(v) => (2, v),
            GridShift::Hex => (3, 0.0),
        };
        GridForm {
            rows: g.rows as f64,
            cols: g.cols as f64,
            c2c: g.centre_to_centre,
            dx: g.dx,
            dy: g.dy,
            shift,
            shift_amount: amount,
            mirror_rows: g.mirror_alt_rows,
            mirror_cols: g.mirror_alt_cols,
            rev_x: g.reverse_x,
            rev_y: g.reverse_y,
            ..GridForm::default()
        }
    }
}

/// Circular array settings.
#[derive(Clone, Debug, PartialEq)]
pub struct CircForm {
    pub cx: f64,
    pub cy: f64,
    pub count: f64,
    pub start: f64,
    pub sweep: f64,
    pub rotate: bool,
    pub clockwise: bool,
    pub virtual_array: bool,
    /// The centre has been set (to the work area's centre) already.
    pub placed: bool,
}

impl Default for CircForm {
    fn default() -> Self {
        CircForm { cx: 0.0, cy: 0.0, count: 6.0, start: 0.0, sweep: 360.0, rotate: true, clockwise: false, virtual_array: true, placed: false }
    }
}

impl CircForm {
    pub fn args(&self) -> Value {
        json!({
            "cx": self.cx, "cy": self.cy, "count": self.count.round().max(1.0), "start_angle": self.start, "sweep": self.sweep,
            "rotate_items": self.rotate, "clockwise": self.clockwise, "virtual": self.virtual_array,
        })
    }

    pub fn from_def(c: &CircularDef) -> CircForm {
        CircForm {
            cx: c.cx,
            cy: c.cy,
            count: c.count as f64,
            start: c.start_angle,
            sweep: c.sweep,
            rotate: c.rotate_items,
            clockwise: c.clockwise,
            virtual_array: true,
            placed: true,
        }
    }
}

/// Copy-along-path settings.
#[derive(Clone, Debug, PartialEq)]
pub struct AlongForm {
    pub path: Option<ShapeId>,
    pub use_spacing: bool,
    pub count: f64,
    pub spacing: f64,
    pub rotate: bool,
    pub nearest: bool,
    pub virtual_array: bool,
}

impl Default for AlongForm {
    fn default() -> Self {
        AlongForm { path: None, use_spacing: false, count: 5.0, spacing: 5.0, rotate: true, nearest: false, virtual_array: true }
    }
}

impl AlongForm {
    pub fn args(&self) -> Value {
        let mut a = json!({ "rotate_items": self.rotate, "start_at_nearest": self.nearest, "virtual": self.virtual_array });
        if self.use_spacing {
            a["spacing"] = json!(self.spacing);
        } else {
            a["count"] = json!(self.count.round().max(1.0));
            a["spacing"] = json!(0.0);
        }
        if let Some(p) = self.path {
            a["path_shape"] = json!(p.0);
        }
        a
    }

    pub fn from_def(d: &AlongDef) -> AlongForm {
        AlongForm {
            path: None,
            use_spacing: d.spacing > 1e-9,
            count: d.count as f64,
            spacing: if d.spacing > 1e-9 { d.spacing } else { 5.0 },
            rotate: d.rotate_items,
            nearest: d.start_at_nearest,
            virtual_array: true,
        }
    }
}

/// Nest settings.
#[derive(Clone, Debug, PartialEq)]
pub struct NestForm {
    pub custom: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub spacing: f64,
    pub margin: f64,
    pub step: f64,
    pub mirror: bool,
}

impl Default for NestForm {
    fn default() -> Self {
        NestForm { custom: false, x: 0.0, y: 0.0, w: 300.0, h: 200.0, spacing: 2.0, margin: 0.0, step: 90.0, mirror: false }
    }
}

impl NestForm {
    pub fn args(&self) -> Value {
        let mut a = json!({ "spacing": self.spacing, "margin": self.margin, "rotation_step": self.step, "allow_mirror": self.mirror });
        if self.custom {
            a["x"] = json!(self.x);
            a["y"] = json!(self.y);
            a["width"] = json!(self.w);
            a["height"] = json!(self.h);
        }
        a
    }
}

/// Alignment helper choices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlignForm {
    /// 0 the first selected, 1 the last selected, 2 the work area.
    pub reference: usize,
    /// 0 left, 1 centre, 2 right.
    pub h: Option<usize>,
    /// 0 top, 1 middle, 2 bottom.
    pub v: Option<usize>,
}

impl AlignForm {
    /// The commands to run, in order. For the work area it is the single "move to" command.
    /// (The centre commands are named for the line the centres end on: `align_v_centres` puts
    /// the centres on one vertical line, `align_h_centres` on one horizontal line.)
    pub fn commands(&self) -> Vec<&'static str> {
        if self.reference == 2 {
            return match (self.h, self.v) {
                (Some(h), Some(v)) => vec![
                    [
                        ["arrange.page_upper_left", "arrange.page_top", "arrange.page_upper_right"],
                        ["arrange.page_left", "arrange.page_centre", "arrange.page_right"],
                        ["arrange.page_lower_left", "arrange.page_bottom", "arrange.page_lower_right"],
                    ][v.min(2)][h.min(2)],
                ],
                _ => Vec::new(),
            };
        }
        let mut c = Vec::new();
        if let Some(h) = self.h {
            c.push(["arrange.align_left", "arrange.align_v_centres", "arrange.align_right"][h.min(2)]);
        }
        if let Some(v) = self.v {
            c.push(["arrange.align_top", "arrange.align_h_centres", "arrange.align_bottom"][v.min(2)]);
        }
        c
    }
}

/// Editing a virtual array: its settings, loaded from the array.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrayEditForm {
    pub target: Option<ShapeId>,
    /// 0 grid, 1 circular, 2 along a path.
    pub kind: usize,
    pub grid: GridForm,
    pub circ: CircForm,
    pub along: AlongForm,
    pub items: usize,
}

impl ArrayEditForm {
    pub fn args(&self) -> Value {
        let mut a = match self.kind {
            0 => self.grid.args(),
            1 => self.circ.args(),
            _ => self.along.args(),
        };
        if let Some(t) = self.target {
            a["id"] = json!(t.0);
        }
        a["kind"] = json!(["grid", "circular", "along_path"][self.kind.min(2)]);
        a
    }

    pub fn load(id: ShapeId, def: &ArrayDef) -> ArrayEditForm {
        let mut f = ArrayEditForm { target: Some(id), items: def.source.len(), ..Default::default() };
        match &def.kind {
            ArrayKind::Grid(g) => f.grid = GridForm::from_def(g),
            ArrayKind::Circular(c) => {
                f.kind = 1;
                f.circ = CircForm::from_def(c);
            }
            ArrayKind::AlongPath(a) => {
                f.kind = 2;
                f.along = AlongForm::from_def(a);
            }
        }
        f
    }
}

// ---------------------------------------------------------------- auto-join worker

type JoinGroups = Vec<(usize, Vec<ShapeId>, Vec<BezPath>)>;

/// Auto-join running on a worker thread, so it can show progress and be cancelled.
pub struct JoinJob {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    handle: Option<JoinHandle<Option<Vec<Vec<BezPath>>>>>,
    groups: JoinGroups,
}

impl JoinJob {
    pub fn start(groups: JoinGroups, tol: f64) -> JoinJob {
        Self::start_with(groups, tol, false)
    }

    /// [`JoinJob::start`] with the cancel flag already set (for tests).
    pub fn start_with(groups: JoinGroups, tol: f64, cancelled: bool) -> JoinJob {
        let cancel = Arc::new(AtomicBool::new(cancelled));
        let (done, total) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let inputs: Vec<Vec<BezPath>> = groups.iter().map(|g| g.2.clone()).collect();
        let (c, d, t) = (cancel.clone(), done.clone(), total.clone());
        let handle = std::thread::spawn(move || {
            let mut out = Vec::new();
            for paths in inputs {
                if c.load(Ordering::Relaxed) {
                    return None;
                }
                let joined = cleanup::auto_join(&paths, tol, &mut |n, of| {
                    d.store(n, Ordering::Relaxed);
                    t.store(of, Ordering::Relaxed);
                    !c.load(Ordering::Relaxed)
                })?;
                out.push(joined);
            }
            Some(out)
        });
        JoinJob { cancel, done, total, handle: Some(handle), groups }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// (done, total) of the group being joined.
    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub fn finished(&self) -> bool {
        self.handle.as_ref().is_none_or(|h| h.is_finished())
    }

    /// Wait for the end: the inputs, and the joined paths per group (None when cancelled).
    pub fn take(mut self) -> (JoinGroups, Option<Vec<Vec<BezPath>>>) {
        let result = self.handle.take().and_then(|h| h.join().ok()).flatten();
        (std::mem::take(&mut self.groups), result)
    }
}

// ---------------------------------------------------------------- state

/// State of the shape dialogs and ribbon fields.
pub struct ShapeUi {
    /// Docking: move the whole selection as one block.
    pub dock_as_group: bool,
    /// Docking: shapes inside other selected shapes follow them.
    pub dock_lock_inner: bool,
    /// Radius (mm) of the Round Corners tool; negative rounds inwards.
    pub corner_radius: f64,
    /// The dialog that is open: the command id that opened it.
    pub open: Option<&'static str>,
    /// Preview outlines drawn over the canvas while a dialog is open (document mm).
    pub preview: Vec<BezPath>,
    /// Coloured outlines (resize slots).
    pub highlights: Vec<(BezPath, Color32)>,
    /// What the last command reported ("Nodes 12 to 8", an error), for the status bar.
    pub status: Option<String>,
    pub offset: OffsetForm,
    pub boolean: BoolForm,
    pub slots: SlotForm,
    pub close_tol: f64,
    pub join_tol: f64,
    pub join_job: Option<JoinJob>,
    pub grid: GridForm,
    pub circ: CircForm,
    pub along: AlongForm,
    pub nest: NestForm,
    pub align: AlignForm,
    pub array_edit: ArrayEditForm,
    /// The error of the dialog's last attempt, and a one-line result.
    pub error: Option<String>,
    pub note: Option<String>,
    /// Slot readout lines of the resize slots dialog.
    pub slot_lines: Vec<String>,
    preview_key: String,
    last_open: Option<&'static str>,
    seen_status: Option<(String, f64)>,
}

impl Default for ShapeUi {
    fn default() -> Self {
        ShapeUi {
            dock_as_group: false,
            dock_lock_inner: false,
            corner_radius: 1.0,
            open: None,
            preview: Vec::new(),
            highlights: Vec::new(),
            status: None,
            offset: OffsetForm::default(),
            boolean: BoolForm::default(),
            slots: SlotForm::default(),
            close_tol: 0.1,
            join_tol: 0.1,
            join_job: None,
            grid: GridForm::default(),
            circ: CircForm::default(),
            along: AlongForm::default(),
            nest: NestForm::default(),
            align: AlignForm::default(),
            array_edit: ArrayEditForm::default(),
            error: None,
            note: None,
            slot_lines: Vec::new(),
            preview_key: String::new(),
            last_open: None,
            seen_status: None,
        }
    }
}

impl ShapeUi {
    /// Open the dialog of command `id`.
    pub fn open_dialog(&mut self, id: &'static str) {
        self.open = Some(id);
    }

    pub fn close(&mut self) {
        self.open = None;
        self.clear_preview();
    }

    fn clear_preview(&mut self) {
        self.preview.clear();
        self.highlights.clear();
        self.preview_key.clear();
    }

    /// Reset what a dialog shows when a different one opens.
    fn on_open(&mut self, id: Option<&'static str>) {
        self.clear_preview();
        self.error = None;
        self.note = None;
        self.slot_lines.clear();
        if let Some(job) = self.join_job.take() {
            job.cancel();
        }
        match id {
            Some("tools.boolean_assistant") => self.boolean = BoolForm::default(),
            Some("arrange.circular_array") => self.circ.placed = false,
            Some("arrange.copy_along_path") => self.along.path = None,
            Some("arrange.array_edit") => self.array_edit.target = None,
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- previews

fn page_rect(app: &DarkBurn) -> Rect {
    Rect::new(0.0, 0.0, app.prefs.page_width as f64, app.prefs.page_height as f64)
}

fn grid_def_for(args: &Value, source: &[darkburn_core::Shape]) -> GridDef {
    let mut g = crate::shape_arrays::grid_from(args, &GridDef::default());
    if args.get("fit_width").is_some()
        && let Some(bb) = varray::bbox_of(source)
    {
        let w = crate::shape_ops::num(args, "fit_width", bb.width());
        let h = crate::shape_ops::num(args, "fit_height", bb.height());
        let (rows, cols) = garray::grid_fit(bb, Rect::new(bb.x0, bb.y0, bb.x0 + w, bb.y0 + h), g.options().spacing);
        g.rows = rows.max(1);
        g.cols = cols.max(1);
    }
    g
}

/// The outlines an array definition would make (empty when it would be huge).
pub fn array_outlines(def: &ArrayDef) -> Vec<BezPath> {
    if def.transforms().len() * def.source.len().max(1) > PREVIEW_COPIES {
        return Vec::new();
    }
    crate::shape_arrays::array_preview(def)
}

fn selection_source(e: &Editor, skip: Option<ShapeId>) -> Vec<darkburn_core::Shape> {
    crate::shape_ops::in_draw_order(&e.doc, &e.selected()).into_iter().filter(|i| Some(*i) != skip).filter_map(|i| e.doc.get(i).cloned()).collect()
}

const PALETTE: [Color32; 6] = [
    Color32::from_rgb(255, 150, 30),
    Color32::from_rgb(60, 200, 90),
    Color32::from_rgb(70, 140, 255),
    Color32::from_rgb(230, 80, 200),
    Color32::from_rgb(40, 210, 220),
    Color32::from_rgb(240, 220, 50),
];

/// Group label of a slot: kind, notch or tab, and its size to 0.01 mm.
fn slot_group(s: &darkburn_geom::slots::Slot) -> String {
    let kind = match s.kind {
        SlotKind::Rect => "rectangular",
        SlotKind::Round => "round",
    };
    let what = if s.protruding { "tab" } else { "slot" };
    format!("{kind} {what} {:.2} x {:.2} mm", s.width, s.depth)
}

/// Coloured outlines and readout lines for the slots found with `form` in `ids`.
pub fn slot_highlights(e: &Editor, ids: &[ShapeId], form: &SlotForm) -> (Vec<(BezPath, Color32)>, Vec<String>) {
    let q = crate::shape_ops::slot_query(&form.query_args());
    let hits = crate::shape_ops::find_slots_in(&e.doc, ids, &q);
    let mut groups: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::new();
    for h in &hits {
        let key = slot_group(&h.slot);
        let i = match groups.iter().position(|g| g.0 == key) {
            Some(i) => {
                groups[i].1 += 1;
                i
            }
            None => {
                groups.push((key, 1));
                groups.len() - 1
            }
        };
        out.push((h.quad.clone(), PALETTE[i % PALETTE.len()]));
    }
    let lines = groups.iter().map(|(k, n)| format!("{n} x {k}")).collect();
    (out, lines)
}

fn refresh(app: &mut DarkBurn, id: &str) {
    let ui = &app.shape_ui;
    let key = format!(
        "{id}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        app.editor.revision(),
        app.editor.selected(),
        ui.offset,
        ui.boolean,
        ui.slots,
        ui.grid,
        ui.circ,
        ui.along,
        ui.array_edit,
        app.prefs.page_width,
    );
    if key == ui.preview_key {
        return;
    }
    let e = &app.editor;
    let sel = e.selected();
    let (mut preview, mut highlights, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    match id {
        "tools.offset" => {
            if let Ok(p) = crate::shape_ops::OffsetParams::from_json(&ui.offset.args()) {
                preview = crate::shape_ops::offset_preview(&e.doc, &sel, &p);
            }
        }
        "tools.boolean_assistant" => {
            let b = &ui.boolean;
            if !b.a.is_empty() && !b.b.is_empty() {
                let a = b.args();
                let ids = |k: &str| crate::shape_ops::ids_arg(&a, k).unwrap_or_default();
                preview = crate::shape_ops::boolean_preview(&e.doc, &ids("a"), &ids("b"), a["op"].as_str().unwrap_or("union"));
            }
        }
        "tools.resize_slots" => {
            let (h, l) = slot_highlights(e, &sel, &ui.slots);
            highlights = h;
            lines = l;
        }
        "arrange.grid_array" => {
            let source = selection_source(e, None);
            if !source.is_empty() {
                let g = grid_def_for(&ui.grid.args(), &source);
                preview = array_outlines(&ArrayDef { source, kind: ArrayKind::Grid(g) });
            }
        }
        "arrange.circular_array" => {
            let source = selection_source(e, None);
            if !source.is_empty() {
                let c = crate::shape_arrays::circ_from(&ui.circ.args(), &CircularDef::default());
                preview = array_outlines(&ArrayDef { source, kind: ArrayKind::Circular(c) });
            }
        }
        "arrange.copy_along_path" => {
            let path_id = ui.along.path.or(sel.last().copied());
            if let Some(pid) = path_id
                && let Some(path) = crate::shape_ops::shape_path(&e.doc, pid)
            {
                let source = selection_source(e, Some(pid));
                if !source.is_empty() {
                    let a = crate::shape_arrays::along_from(&ui.along.args(), &AlongDef { path, ..AlongDef::default() });
                    preview = array_outlines(&ArrayDef { source, kind: ArrayKind::AlongPath(a) });
                }
            }
        }
        "arrange.array_edit" => {
            if let Some(t) = ui.array_edit.target
                && let Some(s) = e.doc.get(t)
                && let Some(def) = s.array.as_deref()
            {
                let a = ui.array_edit.args();
                let kind = match (&def.kind, ui.array_edit.kind) {
                    (ArrayKind::Grid(_), 0) => ArrayKind::Grid(grid_def_for(&a, &def.source)),
                    (ArrayKind::Circular(c), 1) => ArrayKind::Circular(crate::shape_arrays::circ_from(&a, c)),
                    (ArrayKind::AlongPath(p), 2) => ArrayKind::AlongPath(crate::shape_arrays::along_from(&a, p)),
                    (k, _) => k.clone(),
                };
                let t_all = s.transform;
                preview = array_outlines(&ArrayDef { source: def.source.clone(), kind })
                    .into_iter()
                    .map(|mut p| {
                        p.apply_affine(t_all);
                        p
                    })
                    .collect();
            }
        }
        _ => {}
    }
    let ui = &mut app.shape_ui;
    ui.preview = preview;
    ui.highlights = highlights;
    ui.slot_lines = lines;
    ui.preview_key = key;
}

/// Draw the dialogs' preview outlines over the canvas.
pub fn paint_preview(app: &DarkBurn, xf: &crate::render::Xf, painter: &egui::Painter, accent: egui::Color32) {
    for path in &app.shape_ui.preview {
        for (mut pts, closed) in crate::render::screen_polylines(path, xf, true) {
            if closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, accent)));
        }
    }
    for (path, colour) in &app.shape_ui.highlights {
        for (pts, _) in crate::render::screen_polylines(path, xf, true) {
            if pts.len() >= 3 {
                painter.add(egui::Shape::convex_polygon(pts, colour.gamma_multiply(0.25), egui::Stroke::new(2.0, *colour)));
            }
        }
    }
}

// ---------------------------------------------------------------- running commands

/// Run command `id` with `args`; errors land in the dialog and the status. True on success.
fn run_command(app: &mut DarkBurn, ctx: &Context, id: &str, args: &Value) -> bool {
    match crate::api::execute(app, ctx, id, args) {
        Ok(v) => {
            app.shape_ui.error = None;
            app.shape_ui.note = result_note(id, &v);
            app.shape_ui.status = app.shape_ui.note.clone();
            true
        }
        Err(m) => {
            app.shape_ui.error = Some(m.clone());
            app.shape_ui.status = Some(m);
            false
        }
    }
}

fn result_note(id: &str, v: &Value) -> Option<String> {
    let n = |k: &str| v.get(k).and_then(Value::as_u64);
    match id {
        "arrange.nest" => Some(format!("Placed {}, {} did not fit", n("placed")?, n("unplaced")?)),
        "tools.resize_slots" => Some(format!("Resized {} slots", n("resized")?)),
        _ => None,
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Act {
    None,
    /// Run and keep the dialog open.
    Apply,
    /// Run and close.
    Ok,
    Cancel,
}

// ---------------------------------------------------------------- widgets

fn length() -> Num {
    Num::new(-100_000.0..=100_000.0, 2, " mm").speed(0.1)
}

fn positive() -> Num {
    Num::new(0.0..=100_000.0, 2, " mm").speed(0.1)
}

fn whole() -> Num {
    Num::new(1.0..=10_000.0, 0, "").speed(0.2)
}

fn angle() -> Num {
    Num::new(-36_000.0..=36_000.0, 2, " deg").speed(0.5)
}

fn num_row(ui: &mut Ui, label: &str, v: &mut f64, num: &Num) {
    numeric::row(ui, label, v, num);
}

fn pick(ui: &mut Ui, id: &str, label: &str, value: &mut usize, options: &[&str]) {
    ui_kit::row(ui, label, |ui| ui_kit::dropdown(ui, id, value, options));
}

fn buttons(ui: &mut Ui, ok_label: &str, with_apply: bool, can: bool) -> Act {
    let mut act = Act::None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(can, egui::Button::new(ok_label)).clicked() {
            act = Act::Ok;
        }
        if with_apply && ui.add_enabled(can, egui::Button::new("Apply")).clicked() {
            act = Act::Apply;
        }
        if ui.button("Cancel").clicked() {
            act = Act::Cancel;
        }
    });
    act
}

fn show_error(ui: &mut Ui, app: &DarkBurn) {
    if let Some(e) = &app.shape_ui.error {
        ui.colored_label(Color32::from_rgb(225, 80, 70), e);
    } else if let Some(n) = &app.shape_ui.note {
        ui.label(n);
    }
}

/// A non-modal window the user can keep open while working, so it can also be docked as a tab
/// or put in a window of its own (its OK, Apply and Cancel are in the body); returns false when
/// it was closed (its close button, Escape, or its tab).
fn window(ctx: &Context, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let screen = ctx.content_rect();
    let id = Id::unique(("shape_dialog", title));
    let key = dock_key(title);
    let (closed, _) = ui_kit::Window::new(id, "db_app", title)
        .dockable(&key)
        .body_width(300.0)
        .placed(egui::Align2::LEFT_TOP, pos2((screen.right() - 340.0).max(0.0), 120.0))
        .actions("", "", None)
        // Escape closes the dialog, as it does for every other window in the app.
        .escape_here(crate::escape::only_window(ctx, id))
        .show(ctx, true, add);
    !closed
}

/// The stored-layout key of the shape dialog titled `title`.
pub fn dock_key(title: &str) -> String {
    let slug: String = title.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    format!("db_shape_{slug}")
}

fn selected_count(app: &DarkBurn) -> usize {
    app.editor.selection.len()
}

/// Finish a dialog frame: act on the button pressed.
fn finish(app: &mut DarkBurn, ctx: &Context, id: &str, act: Act, args: Value, still_open: bool) {
    if !still_open || act == Act::Cancel {
        app.shape_ui.close();
        return;
    }
    match act {
        Act::Apply => {
            run_command(app, ctx, id, &args);
        }
        Act::Ok if run_command(app, ctx, id, &args) => {
            app.shape_ui.close();
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- shared array form widgets

fn grid_form_ui(ui: &mut Ui, f: &mut GridForm, allow_virtual: bool, fixed_size: bool) {
    num_row(ui, "Rows", &mut f.rows, &whole());
    num_row(ui, "Columns", &mut f.cols, &whole());
    if !fixed_size {
        ui_kit::checkbox(ui, "Fill a target size", &mut f.fit);
        if f.fit {
            num_row(ui, "Width", &mut f.fit_w, &positive());
            num_row(ui, "Height", &mut f.fit_h, &positive());
        }
    }
    ui_kit::checkbox(ui, "Spacing is centre to centre", &mut f.c2c);
    num_row(ui, if f.c2c { "Step X" } else { "Gap X" }, &mut f.dx, &length());
    num_row(ui, if f.c2c { "Step Y" } else { "Gap Y" }, &mut f.dy, &length());
    pick(ui, "grid_shift", "Shift", &mut f.shift, &["None", "Alternate rows", "Alternate columns", "Hex layout"]);
    if f.shift == 1 || f.shift == 2 {
        num_row(ui, "Shift by", &mut f.shift_amount, &length());
    }
    ui_kit::checkbox(ui, "Mirror alternate rows", &mut f.mirror_rows);
    ui_kit::checkbox(ui, "Mirror alternate columns", &mut f.mirror_cols);
    ui_kit::checkbox(ui, "Reverse horizontal direction", &mut f.rev_x);
    ui_kit::checkbox(ui, "Reverse vertical direction", &mut f.rev_y);
    if allow_virtual {
        ui_kit::checkbox(ui, "Virtual array (stays editable)", &mut f.virtual_array);
    }
}

fn circ_form_ui(ui: &mut Ui, f: &mut CircForm, allow_virtual: bool) {
    num_row(ui, "Centre X", &mut f.cx, &length());
    num_row(ui, "Centre Y", &mut f.cy, &length());
    num_row(ui, "Count", &mut f.count, &whole());
    num_row(ui, "Start angle", &mut f.start, &angle());
    num_row(ui, "Sweep", &mut f.sweep, &angle());
    ui_kit::checkbox(ui, "Rotate shapes", &mut f.rotate);
    ui_kit::checkbox(ui, "Clockwise", &mut f.clockwise);
    if allow_virtual {
        ui_kit::checkbox(ui, "Virtual array (stays editable)", &mut f.virtual_array);
    }
}

fn along_form_ui(ui: &mut Ui, f: &mut AlongForm, allow_virtual: bool) {
    ui_kit::checkbox(ui, "Space by distance instead of count", &mut f.use_spacing);
    if f.use_spacing {
        num_row(ui, "Spacing", &mut f.spacing, &positive());
    } else {
        num_row(ui, "Count", &mut f.count, &whole());
    }
    ui_kit::checkbox(ui, "Rotate shapes along the path", &mut f.rotate);
    ui_kit::checkbox(ui, "Start at the nearest point", &mut f.nearest);
    if allow_virtual {
        ui_kit::checkbox(ui, "Virtual array (stays editable)", &mut f.virtual_array);
    }
}

// ---------------------------------------------------------------- the dialogs

fn offset_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "tools.offset";
    let mut f = app.shape_ui.offset.clone();
    let mut act = Act::None;
    let can = selected_count(app) > 0;
    let open = window(ctx, "Offset Shapes", |ui| {
        num_row(ui, "Distance", &mut f.distance, &length());
        pick(ui, "off_side", "Direction", &mut f.side, &["Outward", "Inward", "Both"]);
        pick(ui, "off_join", "Corners", &mut f.join, &["Round", "Bevel", "Sharp"]);
        ui_kit::checkbox(ui, "Delete original", &mut f.delete_original);
        ui_kit::checkbox(ui, "Select results", &mut f.select_results);
        ui_kit::checkbox(ui, "Outer shapes only", &mut f.outer_only);
        ui_kit::checkbox(ui, "Offset open shapes", &mut f.open_shapes);
        if !can {
            ui.label("Select the shapes to offset.");
        }
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.offset = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        refresh(app, id);
    }
}

fn boolean_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "tools.boolean_assistant";
    let mut f = app.shape_ui.boolean.clone();
    let sel = app.editor.selected();
    let mut act = Act::None;
    let mut reset = false;
    let open = window(ctx, "Boolean Assistant", |ui| {
        ui.label("Select the shapes of set A and press Set A, then the shapes of set B and press Set B.");
        ui.horizontal(|ui| {
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("Set A from selection")).clicked() {
                f.a = sel.clone();
            }
            ui.label(format!("{} shapes", f.a.len()));
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("Set B from selection")).clicked() {
                f.b = sel.clone();
            }
            ui.label(format!("{} shapes", f.b.len()));
        });
        pick(ui, "bool_op", "Operation", &mut f.op, &BOOL_OPS);
        ui_kit::checkbox(ui, "Keep originals", &mut f.keep);
        if ui.button("Reset").clicked() {
            reset = true;
        }
        act = buttons(ui, "OK", true, !f.a.is_empty() && !f.b.is_empty());
        show_error(ui, app);
    });
    if reset {
        f = BoolForm::default();
    }
    let args = f.args();
    app.shape_ui.boolean = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        if act == Act::Apply {
            app.shape_ui.boolean = BoolForm::default();
        }
        refresh(app, id);
    }
}

fn slots_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "tools.resize_slots";
    let mut f = app.shape_ui.slots.clone();
    let lines = app.shape_ui.slot_lines.clone();
    let mut act = Act::None;
    let can = selected_count(app) > 0 && (f.set_w || f.set_d);
    let open = window(ctx, "Resize Slots in Selection", |ui| {
        ui.label("Slots matching the filter are highlighted; slots of one size share a colour.");
        ui_kit::section_heading(ui, "Find (0 means no limit)");
        num_row(ui, "Min width", &mut f.min_w, &positive());
        num_row(ui, "Max width", &mut f.max_w, &positive());
        num_row(ui, "Min depth", &mut f.min_d, &positive());
        num_row(ui, "Max depth", &mut f.max_d, &positive());
        ui_kit::checkbox(ui, "Include protruding tabs", &mut f.include_tabs);
        ui_kit::section_heading(ui, "Resize to");
        ui_kit::checkbox(ui, "Set width", &mut f.set_w);
        if f.set_w {
            num_row(ui, "New width", &mut f.new_w, &positive());
        }
        ui_kit::checkbox(ui, "Set depth", &mut f.set_d);
        if f.set_d {
            num_row(ui, "New depth", &mut f.new_d, &positive());
        }
        ui_kit::section_heading(ui, "Found");
        if lines.is_empty() {
            ui.label("No slots found in the selection.");
        }
        for (i, l) in lines.iter().enumerate() {
            ui.colored_label(PALETTE[i % PALETTE.len()], l);
        }
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.slots = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        refresh(app, id);
    }
}

fn tolerance_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "edit.close_tolerance";
    let mut tol = app.shape_ui.close_tol;
    let mut act = Act::None;
    let can = selected_count(app) > 0;
    let open = window(ctx, "Close Paths with Tolerance", |ui| {
        ui.label("Open paths whose ends are closer than this are closed.");
        num_row(ui, "Tolerance", &mut tol, &Num::new(0.0..=1000.0, 3, " mm").speed(0.01));
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    app.shape_ui.close_tol = tol;
    finish(app, ctx, id, act, json!({ "tolerance": tol }), open);
}

fn auto_join_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut tol = app.shape_ui.join_tol;
    let mut start = false;
    let mut cancel_job = false;
    let mut close = false;
    let running = app.shape_ui.join_job.as_ref().map(JoinJob::progress);
    let can = selected_count(app) > 0;
    let open = window(ctx, "Auto-Join Selected Shapes", |ui| {
        ui.label("Open paths whose ends meet within the tolerance are joined into one. Ends at T-junctions are left alone.");
        ui.add_enabled_ui(running.is_none(), |ui| num_row(ui, "Tolerance", &mut tol, &Num::new(0.0..=1000.0, 3, " mm").speed(0.01)));
        if let Some((done, total)) = running {
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(frac).text(format!("Joining {done} of {total}")));
            if ui.button("Cancel").clicked() {
                cancel_job = true;
            }
        } else {
            ui.horizontal(|ui| {
                if ui.add_enabled(can, egui::Button::new("Join")).clicked() {
                    start = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        }
        show_error(ui, app);
    });
    app.shape_ui.join_tol = tol;
    if !open || close {
        app.shape_ui.close();
        if let Some(j) = app.shape_ui.join_job.take() {
            j.cancel();
        }
        return;
    }
    if cancel_job && let Some(j) = &app.shape_ui.join_job {
        j.cancel();
    }
    if start {
        let groups = crate::shape_ops::join_inputs(&app.editor.doc, &app.editor.selected());
        if groups.is_empty() {
            app.shape_ui.error = Some("select open paths to join".into());
        } else {
            app.shape_ui.error = None;
            app.shape_ui.join_job = Some(JoinJob::start(groups, tol.max(1e-6)));
        }
    }
    if app.shape_ui.join_job.as_ref().is_some_and(JoinJob::finished)
        && let Some(job) = app.shape_ui.join_job.take()
    {
        let (groups, result) = job.take();
        match result {
            Some(r) => match crate::shape_ops::apply_joined(&mut app.editor, &groups, r) {
                Ok(v) => {
                    app.shape_ui.status = Some(format!("Joined {} shapes into {}", v["before"], v["after"]));
                    app.shape_ui.close();
                }
                Err(m) => app.shape_ui.error = Some(m),
            },
            None => {
                app.shape_ui.error = Some("Cancelled, nothing changed".into());
                app.shape_ui.status = app.shape_ui.error.clone();
            }
        }
    }
    if app.shape_ui.join_job.is_some() {
        ctx.request_repaint();
    }
}

fn grid_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "arrange.grid_array";
    let mut f = app.shape_ui.grid.clone();
    let mut act = Act::None;
    let can = selected_count(app) > 0;
    let open = window(ctx, "Grid Array", |ui| {
        grid_form_ui(ui, &mut f, true, false);
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.grid = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        refresh(app, id);
    }
}

fn circular_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "arrange.circular_array";
    let mut f = app.shape_ui.circ.clone();
    if !f.placed {
        let c = page_rect(app).center();
        (f.cx, f.cy, f.placed) = (c.x, c.y, true);
    }
    let sel_centre = app.editor.doc.bbox_of(&app.editor.selected()).map(|b| b.center());
    let mut act = Act::None;
    let can = selected_count(app) > 0;
    let open = window(ctx, "Circular Array", |ui| {
        circ_form_ui(ui, &mut f, true);
        if ui.add_enabled(sel_centre.is_some(), egui::Button::new("Centre on selection")).clicked()
            && let Some(c) = sel_centre
        {
            (f.cx, f.cy) = (c.x, c.y);
        }
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.circ = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        refresh(app, id);
    }
}

fn along_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "arrange.copy_along_path";
    let mut f = app.shape_ui.along.clone();
    let sel = app.editor.selected();
    if f.path.is_none_or(|p| !sel.contains(&p)) {
        f.path = sel.last().copied();
    }
    let mut act = Act::None;
    let can = sel.len() >= 2;
    let open = window(ctx, "Copy Along Path", |ui| {
        if !can {
            ui.label("Select the shapes to copy and the path to follow.");
        }
        ui_kit::row(ui, "Path", |ui| {
            let current = f.path.map(|p| format!("Shape {}", p.0)).unwrap_or_default();
            ui_kit::dropdown_menu(ui, "along_path_pick", &current, |ui| {
                for s in &sel {
                    if ui.selectable_label(f.path == Some(*s), format!("Shape {}", s.0)).clicked() {
                        f.path = Some(*s);
                    }
                }
            });
        });
        along_form_ui(ui, &mut f, true);
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.along = f;
    finish(app, ctx, id, act, args, open);
    if app.shape_ui.open == Some(id) {
        refresh(app, id);
    }
}

fn nest_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "arrange.nest";
    let mut f = app.shape_ui.nest.clone();
    let page = page_rect(app);
    let mut act = Act::None;
    let can = selected_count(app) > 0;
    let open = window(ctx, "Nest Selected", |ui| {
        ui_kit::checkbox(ui, "Custom sheet (else the work area)", &mut f.custom);
        if f.custom {
            num_row(ui, "X", &mut f.x, &length());
            num_row(ui, "Y", &mut f.y, &length());
            num_row(ui, "Width", &mut f.w, &positive());
            num_row(ui, "Height", &mut f.h, &positive());
        } else {
            ui.label(format!("Sheet: {:.0} x {:.0} mm", page.width(), page.height()));
        }
        num_row(ui, "Spacing", &mut f.spacing, &positive());
        num_row(ui, "Margin", &mut f.margin, &positive());
        num_row(ui, "Rotation step", &mut f.step, &Num::new(0.0..=360.0, 1, " deg").speed(1.0));
        ui_kit::checkbox(ui, "Allow mirroring", &mut f.mirror);
        act = buttons(ui, "OK", true, can);
        show_error(ui, app);
    });
    let args = f.args();
    app.shape_ui.nest = f;
    finish(app, ctx, id, act, args, open);
}

/// The "Move selection" pop-up of the Arrange toolbar: nine places on the work area.
const PAGE_SPOTS: [[(&str, &str); 3]; 3] = [
    [("Upper left", "arrange.page_upper_left"), ("Top", "arrange.page_top"), ("Upper right", "arrange.page_upper_right")],
    [("Left", "arrange.page_left"), ("Centre", "arrange.page_centre"), ("Right", "arrange.page_right")],
    [("Lower left", "arrange.page_lower_left"), ("Bottom", "arrange.page_bottom"), ("Lower right", "arrange.page_lower_right")],
];

fn move_selection_popup(ctx: &Context, app: &mut DarkBurn) {
    let can = selected_count(app) > 0;
    let mut chosen: Option<&'static str> = None;
    let open = window(ctx, "Move Selection", |ui| {
        ui.label(if can { "Move the selection to a place on the work area." } else { "Select the shapes to move first." });
        egui::Grid::new("move_selection_spots").spacing([6.0, 6.0]).show(ui, |ui| {
            for row in PAGE_SPOTS {
                for (label, id) in row {
                    if ui.add_enabled(can, egui::Button::new(label).min_size(vec2(84.0, 26.0))).clicked() {
                        chosen = Some(id);
                    }
                }
                ui.end_row();
            }
        });
    });
    if let Some(id) = chosen {
        run_command(app, ctx, id, &Value::Null);
        app.shape_ui.close();
    } else if !open {
        app.shape_ui.close();
    }
}

fn align_dialog(ctx: &Context, app: &mut DarkBurn) {
    let mut f = app.shape_ui.align.clone();
    let n = selected_count(app);
    let mut apply = false;
    let mut close = false;
    let open = window(ctx, "Alignment Helper", |ui| {
        ui.label("Step 1: select the shapes you want to line up. Step 2: choose what they line up with. Step 3: choose how, then press Align.");
        ui_kit::section_heading(ui, "Line up with");
        ui.horizontal_wrapped(|ui| {
            for (i, l) in ["The first shape I selected", "The last shape I selected", "The work area"].iter().enumerate() {
                ui.selectable_value(&mut f.reference, i, *l);
            }
        });
        ui_kit::section_heading(ui, "Sideways");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut f.h, None, "Keep");
            for (i, l) in ["Left edges", "Centres", "Right edges"].iter().enumerate() {
                ui.selectable_value(&mut f.h, Some(i), *l);
            }
        });
        ui_kit::section_heading(ui, "Up and down");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut f.v, None, "Keep");
            for (i, l) in ["Top edges", "Middles", "Bottom edges"].iter().enumerate() {
                ui.selectable_value(&mut f.v, Some(i), *l);
            }
        });
        if f.reference == 2 {
            ui.label("For the work area choose both a sideways and an up-and-down position.");
        }
        let need = if f.reference == 2 { 1 } else { 2 };
        if n < need {
            ui.label(format!("Select at least {need} shape(s)."));
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(n >= need && !f.commands().is_empty(), egui::Button::new("Align")).clicked() {
                apply = true;
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        show_error(ui, app);
    });
    app.shape_ui.align = f.clone();
    if !open || close {
        app.shape_ui.close();
        return;
    }
    if apply {
        if f.reference == 0 {
            // The commands align to the last selected: make the first selected the last.
            let mut ids = app.editor.selected();
            if ids.len() > 1 {
                let first = ids.remove(0);
                ids.push(first);
                app.editor.selection.set(ids);
            }
        }
        for c in f.commands() {
            if !run_command(app, ctx, c, &Value::Null) {
                break;
            }
        }
    }
}

fn array_edit_dialog(ctx: &Context, app: &mut DarkBurn) {
    let id = "arrange.array_edit";
    let sel = app.editor.selected();
    let found = sel.iter().copied().find(|i| app.editor.doc.get(*i).is_some_and(|s| s.array.is_some()));
    if let Some(t) = found
        && app.shape_ui.array_edit.target != Some(t)
        && let Some(def) = app.editor.doc.get(t).and_then(|s| s.array.as_deref())
    {
        app.shape_ui.array_edit = ArrayEditForm::load(t, def);
    }
    let mut f = app.shape_ui.array_edit.clone();
    let items: Vec<String> = f
        .target
        .and_then(|t| app.editor.doc.get(t))
        .and_then(|s| s.array.as_deref())
        .map(|d| d.source.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s.kind_name())).collect())
        .unwrap_or_default();
    let has_target = found.is_some() && !items.is_empty();
    let mut act: Option<(&'static str, Value)> = None;
    let mut exit = false;
    let open = window(ctx, "Edit Virtual Array", |ui| {
        if !has_target {
            ui.label("Select a virtual array to edit it.");
            if ui.button("Exit").clicked() {
                exit = true;
            }
            return;
        }
        ui.label(format!("{} original item(s). Changes show on the canvas before you apply them.", items.len()));
        match f.kind {
            0 => grid_form_ui(ui, &mut f.grid, false, true),
            1 => circ_form_ui(ui, &mut f.circ, false),
            _ => along_form_ui(ui, &mut f.along, false),
        }
        ui_kit::section_heading(ui, "Original items");
        for (i, name) in items.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(name);
                if ui.small_button("Remove").clicked() {
                    act = Some(("arrange.array_remove", json!({ "index": i, "id": f.target.map(|t| t.0) })));
                }
            });
        }
        if ui.add_enabled(sel.len() >= 2, egui::Button::new("Add selected shapes")).on_hover_text("Select the array and the shapes to add").clicked()
        {
            act = Some(("arrange.array_add", json!({ "id": f.target.map(|t| t.0) })));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                act = Some((id, f.args()));
            }
            if ui.button("Duplicate").clicked() {
                act = Some(("arrange.array_duplicate", json!({ "id": f.target.map(|t| t.0), "dx": 10.0, "dy": 10.0 })));
            }
            if ui.button("Flatten").clicked() {
                act = Some(("arrange.array_flatten", Value::Null));
            }
            if ui.button("Exit").clicked() {
                exit = true;
            }
        });
        show_error(ui, app);
    });
    app.shape_ui.array_edit = f;
    if !open || exit {
        app.shape_ui.close();
        let _ = crate::api::execute(app, ctx, "arrange.array_exit", &Value::Null);
        return;
    }
    if let Some((cmd, args)) = act
        && run_command(app, ctx, cmd, &args)
    {
        if cmd == "arrange.array_flatten" {
            app.shape_ui.close();
            return;
        }
        // Load the array again: it may have changed.
        app.shape_ui.array_edit.target = None;
    }
    refresh(app, id);
}

// ---------------------------------------------------------------- entry points

/// The open shape dialogs for one frame.
pub fn show(ctx: &Context, app: &mut DarkBurn) {
    if app.shape_ui.open != app.shape_ui.last_open {
        let open = app.shape_ui.open;
        app.shape_ui.on_open(open);
        app.shape_ui.last_open = open;
    }
    toast(ctx, app);
    match app.shape_ui.open {
        Some("tools.offset") => offset_dialog(ctx, app),
        Some("tools.boolean_assistant") => boolean_dialog(ctx, app),
        Some("tools.resize_slots") => slots_dialog(ctx, app),
        Some("edit.close_tolerance") => tolerance_dialog(ctx, app),
        Some("edit.auto_join") => auto_join_dialog(ctx, app),
        Some("arrange.grid_array") => grid_dialog(ctx, app),
        Some("arrange.circular_array") => circular_dialog(ctx, app),
        Some("arrange.copy_along_path") => along_dialog(ctx, app),
        Some("arrange.nest") => nest_dialog(ctx, app),
        Some("arrange.alignment_helper") => align_dialog(ctx, app),
        Some("arrange.array_edit") => array_edit_dialog(ctx, app),
        Some("arrange.move_selection") => move_selection_popup(ctx, app),
        _ => {}
    }
}

/// Show a new status message for a few seconds in the lower left of the window.
fn toast(ctx: &Context, app: &mut DarkBurn) {
    let now = ctx.input(|i| i.time);
    let ui = &mut app.shape_ui;
    match (&ui.status, &ui.seen_status) {
        (Some(s), Some((seen, _))) if s == seen => {}
        (Some(s), _) => ui.seen_status = Some((s.clone(), now)),
        (None, _) => ui.seen_status = None,
    }
    let Some((text, since)) = ui.seen_status.clone() else { return };
    if now - since > STATUS_SECONDS {
        ui.status = None;
        ui.seen_status = None;
        return;
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let screen = ctx.content_rect();
    egui::Area::new(Id::unique(crate::escape::TOAST))
        .fixed_pos(pos2(screen.left() + 70.0, screen.bottom() - 70.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(text);
            });
        });
}

/// A live ribbon control ("dock_padding", "dock_checks", "corner_radius") at `at`, `height` tall;
/// returns its right edge.
pub fn ribbon_item(ui: &mut Ui, kind: &str, at: Pos2, height: f32, app: &mut DarkBurn) -> f32 {
    let t = Tokens::get();
    let row_h = t.control_height;
    match kind {
        "dock_checks" => {
            let area = egui::Rect::from_min_size(pos2(at.x, at.y + 4.0), vec2(140.0, height));
            ui.scope_builder(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui_kit::checkbox(ui, "Move as group", &mut app.shape_ui.dock_as_group);
                ui_kit::checkbox(ui, "Lock inner objects", &mut app.shape_ui.dock_lock_inner);
            });
            area.right()
        }
        "dock_padding" | "corner_radius" => {
            let (label, label_w, num) = if kind == "dock_padding" {
                ("Padding", 48.0, Num::new(0.0..=1000.0, 2, " mm").speed(0.05))
            } else {
                ("Radius", 44.0, Num::new(-1000.0..=1000.0, 2, " mm").speed(0.05))
            };
            let lr = egui::Rect::from_min_size(pos2(at.x, at.y), vec2(label_w, row_h));
            ui.painter().text(lr.left_center(), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(t.font_small), t.text_dim);
            let fr = egui::Rect::from_min_size(pos2(at.x + label_w, at.y), vec2(80.0, row_h));
            let mut v = if kind == "dock_padding" { app.padding } else { app.shape_ui.corner_radius };
            let ed = ui
                .scope_builder(egui::UiBuilder::new().max_rect(fr).id_salt(("shape_ribbon", kind)), |ui| {
                    numeric::field_sized(ui, &mut v, &num, Some(80.0))
                })
                .inner;
            if ed.changed {
                if kind == "dock_padding" {
                    app.padding = v.max(0.0);
                } else {
                    app.shape_ui.corner_radius = v;
                }
            }
            fr.right()
        }
        _ => at.x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::Prefs;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;

    fn two_open_paths() -> JoinGroups {
        let a = BezPath::from_svg("M0 0 L10 0 L10 10").unwrap();
        let b = BezPath::from_svg("M10 10 L0 10 L0 0.01").unwrap();
        vec![(0, vec![ShapeId(1), ShapeId(2)], vec![a, b])]
    }

    fn frame(ctx: &Context, app: &mut DarkBurn) {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| show(ui.ctx(), app));
        out.textures_delta.clear();
    }

    #[test]
    fn auto_join_job_completes_and_applies_in_one_step() {
        let job = JoinJob::start(two_open_paths(), 0.1);
        while !job.finished() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (groups, result) = job.take();
        let result = result.expect("not cancelled");
        assert_eq!(result[0].len(), 1, "the two halves join");
        let mut e = Editor::new();
        let mut ids = Vec::new();
        e.edit("add", |d, _| {
            for p in &groups[0].2 {
                ids.push(d.add(Shape::path(p.clone(), 0)));
            }
            true
        });
        let groups = vec![(0, ids, groups[0].2.clone())];
        let before = e.history.len();
        crate::shape_ops::apply_joined(&mut e, &groups, result).unwrap();
        assert_eq!(e.doc.len(), 1);
        assert_eq!(e.history.len(), before + 1);
    }

    #[test]
    fn a_cancelled_join_gives_nothing() {
        let job = JoinJob::start_with(two_open_paths(), 0.1, true);
        let (groups, result) = job.take();
        assert!(result.is_none());
        assert_eq!(groups.len(), 1, "the inputs come back untouched");
    }

    #[test]
    fn forms_build_command_args() {
        let o = OffsetForm { distance: 2.5, side: 2, join: 2, delete_original: true, ..Default::default() };
        let a = o.args();
        assert_eq!((a["side"].as_str(), a["join"].as_str(), a["distance"].as_f64()), (Some("both"), Some("sharp"), Some(2.5)));
        assert!(crate::shape_ops::OffsetParams::from_json(&a).is_ok());
        let b = BoolForm { a: vec![ShapeId(1)], b: vec![ShapeId(2)], op: 2, keep: true };
        let a = b.args();
        assert_eq!((a["op"].as_str(), a["a"][0].as_u64(), a["b"][0].as_u64()), (Some("difference"), Some(2), Some(1)), "B minus A swaps the sets");
        let s = SlotForm { min_w: 1.0, max_w: 0.0, set_w: true, new_w: 3.0, ..Default::default() };
        let a = s.args();
        assert!(a.get("max_width").is_none() && a["new_width"] == 3.0 && a.get("new_depth").is_none());
        let g = GridForm { fit: true, shift: 1, shift_amount: 4.0, ..Default::default() };
        let a = g.args();
        assert_eq!(a["shift"], "alt_rows");
        assert!(a.get("fit_width").is_some());
        let c = CircForm { count: 8.0, ..Default::default() }.args();
        assert_eq!(c["count"], 8.0);
        let al = AlongForm { use_spacing: true, path: Some(ShapeId(7)), ..Default::default() }.args();
        assert_eq!((al["spacing"].as_f64(), al["path_shape"].as_u64()), (Some(5.0), Some(7)));
        let n = NestForm { custom: true, ..Default::default() }.args();
        assert!(n.get("width").is_some() && NestForm::default().args().get("width").is_none());
    }

    /// Press OK in a dialog: the form's arguments go through the command registry like the real button.
    fn press_ok(app: &mut DarkBurn, ctx: &Context, id: &'static str, args: Value) {
        app.shape_ui.open_dialog(id);
        frame(ctx, app);
        assert_eq!(app.shape_ui.open, Some(id), "{id} opens");
        finish(app, ctx, id, Act::Ok, args, true);
        assert!(app.shape_ui.open.is_none(), "{id} closed after OK: {:?}", app.shape_ui.error);
    }

    fn area(app: &DarkBurn) -> f64 {
        app.editor.doc.iter().map(|s| darkburn_geom::path::area(&s.outline(), darkburn_geom::path::FillRule::NonZero, 0.01).abs()).sum()
    }

    #[test]
    fn ok_in_each_dialog_changes_the_document_and_closes_it() {
        let ctx = Context::default();
        let mut app = DarkBurn::new(Prefs::default());
        let mut ids = Vec::new();
        app.editor.edit("add", |d, sel| {
            ids.push(d.add(Shape::rect(KRect::new(10.0, 10.0, 30.0, 30.0), 0.0, 1)));
            ids.push(d.add(Shape::rect(KRect::new(20.0, 20.0, 50.0, 40.0), 0.0, 1)));
            sel.set([ids[0]]);
            true
        });
        // Offset.
        let a0 = area(&app);
        let f = OffsetForm { distance: 2.0, delete_original: true, ..Default::default() };
        press_ok(&mut app, &ctx, "tools.offset", f.args());
        assert!(area(&app) > a0 + 10.0, "the offset grew the shape");
        assert_eq!(app.editor.doc.len(), 2, "the original was replaced");
        // Boolean assistant: A minus B.
        let sel: Vec<ShapeId> = app.editor.doc.iter().map(|s| s.id).collect();
        let f = BoolForm { a: vec![sel[1]], b: vec![sel[0]], op: 3, keep: false };
        press_ok(&mut app, &ctx, "tools.boolean_assistant", f.args());
        assert_eq!(app.editor.doc.len(), 1, "two shapes became their intersection");
        // Close with tolerance.
        let mut open = BezPath::new();
        open.move_to((100.0, 100.0));
        open.line_to((120.0, 100.0));
        open.line_to((120.0, 120.0));
        open.line_to((100.0, 100.4));
        let line = app.editor.edit("open", |d, sel| {
            let id = d.add(Shape::path(open.clone(), 2));
            sel.set([id]);
            true
        });
        assert!(line);
        press_ok(&mut app, &ctx, "edit.close_tolerance", json!({ "tolerance": 1.0 }));
        let closed = app.editor.doc.iter().filter(|s| s.layer == 2).all(|s| s.is_closed());
        assert!(closed, "the path was closed within the tolerance");
        // Grid array (real copies), circular array and nesting.
        let one = app.editor.doc.iter().next().unwrap().id;
        app.editor.selection.set([one]);
        let n0 = app.editor.doc.len();
        let g = GridForm { rows: 2.0, cols: 3.0, virtual_array: false, ..Default::default() };
        press_ok(&mut app, &ctx, "arrange.grid_array", g.args());
        assert_eq!(app.editor.doc.len(), n0 + 5, "a 2 x 3 grid adds five copies");
        let sel = app.editor.selected();
        assert_eq!(sel.len(), 6, "the grid is selected afterwards");
        let c = CircForm { count: 4.0, ..Default::default() };
        press_ok(&mut app, &ctx, "arrange.circular_array", c.args());
        let ok = app.editor.doc.iter().any(|s| s.array.is_some());
        assert!(ok, "the circular array is a virtual array");
        app.editor.selection.set(app.editor.doc.iter().map(|s| s.id));
        press_ok(&mut app, &ctx, "arrange.nest", NestForm::default().args());
        assert!(app.shape_ui.note.as_deref().is_some_and(|n| n.starts_with("Placed")), "{:?}", app.shape_ui.note);
    }

    #[test]
    fn alignment_helper_chooses_commands() {
        let mut f = AlignForm { reference: 1, h: Some(0), v: Some(2) };
        assert_eq!(f.commands(), ["arrange.align_left", "arrange.align_bottom"]);
        f.h = Some(1);
        f.v = None;
        assert_eq!(f.commands(), ["arrange.align_v_centres"]);
        f.reference = 2;
        assert!(f.commands().is_empty(), "the work area needs both");
        f.v = Some(1);
        assert_eq!(f.commands(), ["arrange.page_centre"]);
        f.h = Some(2);
        f.v = Some(0);
        assert_eq!(f.commands(), ["arrange.page_upper_right"]);
    }

    #[test]
    fn array_form_round_trips_the_definition() {
        let g = GridDef { rows: 3, cols: 4, dx: 5.0, dy: 6.0, shift: GridShift::AltRows(2.0), ..Default::default() };
        let def = ArrayDef { source: vec![Shape::rect(KRect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0)], kind: ArrayKind::Grid(g) };
        let f = ArrayEditForm::load(ShapeId(9), &def);
        assert_eq!((f.kind, f.items), (0, 1));
        let a = f.args();
        assert_eq!(crate::shape_arrays::grid_from(&a, &GridDef::default()), g);
        assert_eq!(a["id"], 9);
    }

    #[test]
    fn dialogs_preview_and_close_without_a_window() {
        let ctx = Context::default();
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.edit("add", |d, sel| {
            let r = d.add(Shape::rect(KRect::new(10.0, 10.0, 30.0, 30.0), 0.0, 1));
            sel.set([r]);
            true
        });
        for id in DIALOG_COMMANDS {
            app.shape_ui.open_dialog(id);
            frame(&ctx, &mut app);
        }
        app.shape_ui.open_dialog("tools.offset");
        frame(&ctx, &mut app);
        assert_eq!(app.shape_ui.preview.len(), 1, "the offset previews its result");
        app.shape_ui.close();
        assert!(app.shape_ui.preview.is_empty());
        app.shape_ui.grid.rows = 2.0;
        app.shape_ui.grid.cols = 3.0;
        app.shape_ui.open_dialog("arrange.grid_array");
        frame(&ctx, &mut app);
        assert_eq!(app.shape_ui.preview.len(), 6);
        let args = app.shape_ui.grid.args();
        let out = crate::api::execute(&mut app, &ctx, "arrange.grid_array", &args).unwrap();
        assert_eq!(out["ids"].as_array().unwrap().len(), 1, "virtual by default");
        // The array editor loads the array it is given and previews its edit.
        app.shape_ui.open_dialog("arrange.array_edit");
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert_eq!(app.shape_ui.array_edit.items, 1);
        assert_eq!(app.shape_ui.preview.len(), 6);
    }

    #[test]
    fn slot_highlights_group_by_size() {
        let mut app = DarkBurn::new(Prefs::default());
        app.editor.edit("add", |d, sel| {
            let p = BezPath::from_svg("M0 0 L40 0 L40 20 L22 20 L22 12 L18 12 L18 20 L0 20 Z").unwrap();
            let id = d.add(Shape::path(p, 0));
            sel.set([id]);
            true
        });
        let (h, lines) = slot_highlights(&app.editor, &app.editor.selected(), &SlotForm::default());
        assert_eq!(h.len(), 1);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("4.00 x 8.00"), "{lines:?}");
    }
}
