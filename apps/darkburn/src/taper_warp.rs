//! Taper warp (OTHER-10): reshape the selected artwork so it comes out right when it is
//! wrapped round a tapered cup or cone in a rotary. The maths is `darkburn_geom::taper`; this
//! file is the Tools menu window with its live preview and the `tools.taper_warp` command that
//! agents and scripts use.
//!
//! How it behaves is this project's own design; the checklist only names the feature:
//! - The sizes are either the object's top and bottom diameter and its height (the artwork
//!   sits centred on the side, or a set distance down from the top edge), or the top and bottom
//!   width the artwork should get and its height.
//! - Simple squeezes each row towards the narrow end about the artwork's centre line; Cone lays
//!   the artwork out on the unrolled cone, so rows become arcs and columns lean to the apex.
//! - The window shows the result on the canvas as the fields change; OK replaces the shapes in
//!   one undo step. Texts become paths, as with the other warp tools. Images cannot be bent and
//!   are refused with a plain message.

use std::sync::Arc;

use darkburn_core::kurbo::{Affine, BezPath, Rect};
use darkburn_core::{Document, Shape, ShapeId, ShapeKind};
use darkburn_geom::taper::{Taper, TaperError, TaperMode, TaperWarp};
use darkburn_ui::ui_kit;
use egui::{Color32, Stroke};
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::numeric::{self, Num};
use crate::render::{self, Xf};
use crate::shell::DarkBurn;

/// The command (Tools menu).
pub const ID: &str = "tools.taper_warp";

/// The command's arguments as JSON Schema.
pub const ARGS: &str = r#"{"type":"object","properties":{"sizes":{"enum":["diameters","widths"],"description":"diameters (default): top, bottom and height are the object's top and bottom diameter and its height. widths: top and bottom are the widths the artwork's top and bottom edge get, height its new height"},"top":{"type":"number","description":"mm"},"bottom":{"type":"number","description":"mm"},"height":{"type":"number","description":"mm; needed with diameters, defaults to the artwork's height with widths"},"from_top":{"type":"number","description":"diameters only: mm along the side from the object's top edge to the artwork's top edge (default: centred on the side)"},"mode":{"enum":["simple","cone"],"description":"simple (default) squeezes each row towards the narrow end; cone unrolls the cone, so rows become arcs"},"preview":{"type":"boolean","description":"report the sizes without changing anything"}},"required":["top","bottom"]}"#;

/// How closely (mm) the preview follows the result.
const PREVIEW_TOL: f64 = 0.05;

/// The largest size (mm) the window and the command take. Far larger sizes would bend small
/// artwork into paths of millions of points.
pub const MAX_SIZE: f64 = 10_000.0;

/// The arguments of command `id`, when it is ours.
pub fn args_schema(id: &str) -> Option<&'static str> {
    handles(id).then_some(ARGS)
}

/// Whether `id` is ours.
pub fn handles(id: &str) -> bool {
    id == ID
}

// ================================================================ pure parts

/// The sizes the person gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sizes {
    /// The object's top and bottom diameter and height; `from_top` places the artwork that far
    /// down the side (`None` centres it).
    Diameters { top: f64, bottom: f64, height: f64, from_top: Option<f64> },
    /// The widths the artwork's top and bottom edge get, and its new height (`None` keeps it).
    Widths { top: f64, bottom: f64, height: Option<f64> },
}

/// Everything a taper warp needs besides the artwork.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub sizes: Sizes,
    pub mode: TaperMode,
}

impl Settings {
    /// The warp for artwork in the box `src`.
    pub fn warp(&self, src: Rect) -> Result<TaperWarp, TaperError> {
        let taper = match self.sizes {
            Sizes::Diameters { top, bottom, height, from_top } => Taper::from_object(src, top, bottom, height, from_top)?,
            Sizes::Widths { top, bottom, height } => Taper { top, bottom, height: height.unwrap_or(src.height()) },
        };
        TaperWarp::new(src, taper, self.mode)
    }
}

/// What to tell the person when the sizes do not make a warp.
pub fn message(e: TaperError) -> String {
    match e {
        TaperError::BadSize => tr("taper.bad_size", "Every size must be greater than zero."),
        TaperError::EmptyArtwork => tr("taper.flat", "The artwork needs both width and height to be warped."),
        TaperError::OffObject => tr("taper.off_object", "The artwork does not fit on the side of the object there."),
        TaperError::WrapsAround => tr("taper.wraps", "The artwork would go more than once round the object."),
    }
}

/// The shapes of `ids` that can be warped (locked ones are left out). Refused when there are
/// none or when any of them is or holds an image.
pub fn targets(doc: &Document, ids: &[ShapeId]) -> Result<Vec<ShapeId>, String> {
    let ids = crate::shape_ops::in_draw_order(doc, ids);
    if ids.is_empty() {
        return Err(tr("taper.select", "Select the shapes to warp."));
    }
    if ids.iter().filter_map(|i| doc.get(*i)).any(|s| s.leaves().iter().any(Shape::is_image)) {
        return Err(tr("taper.images", "Images cannot be taper warped. Leave them out of the selection."));
    }
    Ok(ids)
}

/// Warp shape `s` whose parent sits at `parent` (document space). Groups are walked so each
/// part is warped where it really is; the result goes back into the parent's space.
fn warp_shape(s: &mut Shape, parent: Affine, w: &TaperWarp, tol: f64) -> usize {
    if let ShapeKind::Group { children } = &mut s.kind {
        let here = parent * s.transform;
        let n = children.iter_mut().map(|c| warp_shape(c, here, w, tol)).sum();
        if n > 0 {
            s.array = None;
        }
        return n;
    }
    if parent.determinant().abs() < 1e-12 {
        return 0;
    }
    let back = parent.inverse();
    crate::shape_ops::map_shape(s, &mut |leaf| {
        let mut p = leaf.outline();
        if darkburn_geom::path::contours(&p).is_empty() {
            return None;
        }
        p.apply_affine(parent);
        let mut q = w.warp_path(&p, tol);
        q.apply_affine(back);
        Some(q)
    })
}

/// Warp the shapes `ids` of `doc` (none of them locked or an image), all with the warp `w`.
pub fn warp_shapes(doc: &mut Document, ids: &[ShapeId], w: &TaperWarp, tol: f64) -> usize {
    let mut n = 0;
    for id in ids {
        if let Some(s) = doc.get_mut(*id) {
            n += warp_shape(s, Affine::IDENTITY, w, tol);
        }
    }
    n
}

/// What the preview shows: the sizes and the warped outlines in document space.
#[derive(Clone, Debug)]
pub struct Preview {
    pub taper: Taper,
    pub identity: bool,
    pub paths: Vec<BezPath>,
}

/// The preview for the selection `ids` of `doc`.
pub fn preview(doc: &Document, ids: &[ShapeId], settings: &Settings) -> Result<Preview, String> {
    let ids = targets(doc, ids)?;
    let doc = crate::text_render::prepare_for_output(doc);
    let src = doc.bbox_of(&ids).ok_or_else(|| tr("taper.select", "Select the shapes to warp."))?;
    let w = settings.warp(src).map_err(message)?;
    let paths = ids.iter().flat_map(|i| crate::shape_ops::parts_of(&doc, *i)).map(|p| w.warp_path(&p.path, PREVIEW_TOL)).collect();
    Ok(Preview { taper: w.taper(), identity: w.is_identity(), paths })
}

/// Warp the selection as one undo step. Equal sizes change nothing and make no step.
pub fn apply(app: &mut DarkBurn, settings: &Settings) -> Result<Value, String> {
    let ids = targets(&app.editor.doc, &app.editor.selected())?;
    let mut error = None;
    let mut taper = None;
    let mut identity = false;
    let mut n = 0;
    let done = app.editor.edit("Taper warp", |d, _| {
        crate::text_render::refresh(d);
        let Some(src) = d.bbox_of(&ids) else { return false };
        let w = match settings.warp(src) {
            Ok(w) => w,
            Err(e) => {
                error = Some(e);
                return false;
            }
        };
        taper = Some(w.taper());
        if w.is_identity() {
            identity = true;
            return false;
        }
        n = warp_shapes(d, &ids, &w, crate::shape_ops::TOL);
        n > 0
    });
    if let Some(e) = error {
        return Err(message(e));
    }
    let Some(t) = taper else { return Err(tr("taper.select", "Select the shapes to warp.")) };
    let sizes = json!({ "top": t.top, "bottom": t.bottom, "height": t.height });
    if identity {
        return Ok(json!({ "changed": false, "warped": 0, "sizes": sizes }));
    }
    if !done {
        return Err(if n == 0 {
            tr("taper.nothing", "There is nothing to warp in the selection.")
        } else {
            tr("taper.too_far", "The warped shapes would reach too far.")
        });
    }
    Ok(json!({ "changed": true, "warped": n, "sizes": sizes }))
}

// ================================================================ agent command

fn number(a: &Value, key: &str) -> Result<Option<f64>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64().filter(|v| v.is_finite()) {
            Some(v) if v > MAX_SIZE => Err(format!("argument {key} must be at most {MAX_SIZE} mm")),
            Some(v) => Ok(Some(v)),
            None => Err(format!("argument {key} must be a number")),
        },
    }
}

fn needed(a: &Value, key: &str) -> Result<f64, String> {
    number(a, key)?.ok_or_else(|| format!("argument {key} is needed"))
}

fn word<'a>(a: &'a Value, key: &str, default: &'a str) -> Result<&'a str, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_str().ok_or_else(|| format!("argument {key} must be text")),
    }
}

/// The settings the arguments `a` give.
pub fn settings_from(a: &Value) -> Result<Settings, String> {
    let mode = match word(a, "mode", "simple")? {
        "simple" => TaperMode::Simple,
        "cone" => TaperMode::Cone,
        other => return Err(format!("mode must be simple or cone, not {other}")),
    };
    let sizes = match word(a, "sizes", "diameters")? {
        "diameters" => Sizes::Diameters {
            top: needed(a, "top")?,
            bottom: needed(a, "bottom")?,
            height: number(a, "height")?.ok_or("argument height (the object's height) is needed with diameters")?,
            from_top: number(a, "from_top")?,
        },
        "widths" => Sizes::Widths { top: needed(a, "top")?, bottom: needed(a, "bottom")?, height: number(a, "height")? },
        other => return Err(format!("sizes must be diameters or widths, not {other}")),
    };
    Ok(Settings { sizes, mode })
}

/// The agent path.
pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    handles(id).then(|| command(app, args))
}

fn command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let settings = settings_from(a)?;
    let preview_only = match a.get("preview") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or("argument preview must be true or false")?,
    };
    if preview_only {
        let p = preview(&app.editor.doc, &app.editor.selected(), &settings)?;
        return Ok(json!({ "changed": false, "warped": 0, "sizes": { "top": p.taper.top, "bottom": p.taper.bottom, "height": p.taper.height } }));
    }
    apply(app, &settings)
}

// ================================================================ the window

/// The window's fields, kept between frames (and between openings, for the diameters).
#[derive(Clone, Debug)]
pub struct Form {
    pub open: bool,
    /// Sizes are the artwork's widths rather than the object's diameters.
    pub widths: bool,
    pub cone: bool,
    pub top_d: f64,
    pub bottom_d: f64,
    pub object_h: f64,
    pub centred: bool,
    pub from_top: f64,
    pub top_w: f64,
    pub bottom_w: f64,
    pub art_h: f64,
    /// The selection and design revision the widths were last fitted to, and the widths put in
    /// then. While the widths are as they were put in, they follow the selection.
    fit_key: String,
    fitted: [f64; 3],
    pub message: Option<String>,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            open: false,
            widths: false,
            cone: false,
            top_d: 80.0,
            bottom_d: 60.0,
            object_h: 100.0,
            centred: true,
            from_top: 0.0,
            top_w: 0.0,
            bottom_w: 0.0,
            art_h: 0.0,
            fit_key: String::new(),
            fitted: [0.0; 3],
            message: None,
        }
    }
}

impl Form {
    /// The settings the fields give.
    pub fn settings(&self) -> Settings {
        let sizes = if self.widths {
            Sizes::Widths { top: self.top_w, bottom: self.bottom_w, height: Some(self.art_h) }
        } else {
            Sizes::Diameters { top: self.top_d, bottom: self.bottom_d, height: self.object_h, from_top: (!self.centred).then_some(self.from_top) }
        };
        Settings { sizes, mode: if self.cone { TaperMode::Cone } else { TaperMode::Simple } }
    }

    /// Fill the widths from the artwork's box (no change until they are edited), and make the
    /// object tall enough for the artwork.
    fn fit(&mut self, src: Rect) {
        self.fit_widths(src);
        if src.height() >= self.object_h {
            self.object_h = (src.height() * 1.25).ceil();
        }
    }

    fn fit_widths(&mut self, src: Rect) {
        self.top_w = src.width();
        self.bottom_w = src.width();
        self.art_h = src.height();
        self.fitted = [self.top_w, self.bottom_w, self.art_h];
    }

    /// When the selection or the design changed since the widths were fitted, fit them to the
    /// new selection, unless the person has typed widths of their own.
    fn follow(&mut self, app: &DarkBurn) {
        let key = fit_key(app);
        if key == self.fit_key {
            return;
        }
        self.fit_key = key;
        if [self.top_w, self.bottom_w, self.art_h] == self.fitted
            && let Some(src) = selection_box(app)
        {
            self.fit_widths(src);
        }
    }
}

fn fit_key(app: &DarkBurn) -> String {
    format!("{}|{:?}", app.editor.revision(), app.editor.selected())
}

/// The box round the selection's shapes that can be warped.
fn selection_box(app: &DarkBurn) -> Option<Rect> {
    let ids = targets(&app.editor.doc, &app.editor.selected()).ok()?;
    crate::text_render::prepare_for_output(&app.editor.doc).bbox_of(&ids)
}

fn form_id() -> egui::Id {
    egui::Id::unique("db_taper_warp_form")
}

fn cache_id() -> egui::Id {
    egui::Id::unique("db_taper_warp_preview")
}

/// The window's fields (the defaults when it was never opened).
pub fn form(ctx: &egui::Context) -> Form {
    ctx.data(|d| d.get_temp::<Form>(form_id())).unwrap_or_default()
}

fn store(ctx: &egui::Context, f: Form) {
    ctx.data_mut(|d| d.insert_temp(form_id(), f));
}

/// The preview, worked out again only when the fields, the design or the selection change.
#[derive(Clone)]
struct Cache {
    key: String,
    result: Arc<Result<Preview, String>>,
}

/// The preview the window shows right now, if it is open.
pub fn current_preview(ctx: &egui::Context) -> Option<Arc<Result<Preview, String>>> {
    ctx.data(|d| d.get_temp::<Cache>(cache_id())).map(|c| c.result)
}

fn cached_preview(ctx: &egui::Context, app: &DarkBurn, settings: &Settings) -> Arc<Result<Preview, String>> {
    let selected = app.editor.selected();
    let key = format!("{settings:?}|{}|{selected:?}", app.editor.revision());
    if let Some(c) = ctx.data(|d| d.get_temp::<Cache>(cache_id()))
        && c.key == key
    {
        return c.result;
    }
    let result = Arc::new(preview(&app.editor.doc, &selected, settings));
    ctx.data_mut(|d| d.insert_temp(cache_id(), Cache { key, result: result.clone() }));
    result
}

fn forget_preview(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Cache>(cache_id()));
}

/// Run from the menu: open the window for the selection.
pub fn run(app: &mut DarkBurn, ctx: &egui::Context, id: &str) -> bool {
    if !handles(id) {
        return false;
    }
    let mut f = form(ctx);
    f.open = true;
    f.message = None;
    f.fit_key = fit_key(app);
    if let Some(src) = selection_box(app) {
        f.fit(src);
    }
    store(ctx, f);
    forget_preview(ctx);
    true
}

/// The window, every frame while it is open.
pub fn window(ctx: &egui::Context, app: &mut DarkBurn) {
    let mut f = form(ctx);
    if !f.open {
        return;
    }
    f.follow(app);
    let settings = f.settings();
    let result = cached_preview(ctx, app, &settings);
    let title = tr("taper.title", "Taper Warp");
    let ok = tr("ui.ok", "OK");
    let cancel = tr("ui.cancel", "Cancel");
    let room = crate::shell::room_under_the_bar(ctx);
    let pos = room.left_top() + egui::vec2(crate::shell::TOOLS_WIDTH + 24.0, 24.0);
    let (closed, action) = ui_kit::Window::new(egui::Id::unique("db_taper_warp"), "offset", &title)
        .fields()
        .actions(&ok, &cancel, None)
        .placed(egui::Align2::LEFT_TOP, pos)
        .show(ctx, result.is_ok(), |ui| body(ui, &mut f, &result));
    if action == ui_kit::Action::Ok {
        match apply(app, &settings) {
            Ok(_) => f.open = false,
            Err(m) => f.message = Some(m),
        }
    }
    if closed || action == ui_kit::Action::Cancel {
        f.open = false;
    }
    if !f.open {
        f.message = None;
        forget_preview(ctx);
    }
    store(ctx, f);
}

fn size_row(ui: &mut egui::Ui, id: &str, label: &str, value: &mut f64, min: f64) {
    ui.push_id(id, |ui| numeric::row(ui, label, value, &Num::new(min..=MAX_SIZE, 2, " mm")));
}

fn body(ui: &mut egui::Ui, f: &mut Form, result: &Result<Preview, String>) {
    ui.set_min_width(300.0);
    ui.add(
        egui::Label::new(tr(
            "taper.help",
            "Reshapes the selection so it looks right on a tapered cup or cone. The wide end keeps the artwork's width.",
        ))
        .wrap(),
    );
    let mut sizes = f.widths as usize;
    let diameters = tr("taper.diameters", "Object diameters");
    let widths = tr("taper.widths", "Artwork widths");
    if ui_kit::segmented(ui, &mut sizes, &[&diameters, &widths], ui_kit::Segments::default()) {
        f.widths = sizes == 1;
    }
    let mut mode = f.cone as usize;
    let simple = tr("taper.simple", "Simple");
    let cone = tr("taper.cone", "Cone");
    ui_kit::row(ui, &tr("taper.mode", "Warp"), |ui| {
        if ui_kit::segmented(ui, &mut mode, &[&simple, &cone], ui_kit::Segments::default()) {
            f.cone = mode == 1;
        }
    });
    if f.widths {
        size_row(ui, "taper_top_w", &tr("taper.top_width", "Top width"), &mut f.top_w, 0.01);
        size_row(ui, "taper_bottom_w", &tr("taper.bottom_width", "Bottom width"), &mut f.bottom_w, 0.01);
        size_row(ui, "taper_art_h", &tr("taper.art_height", "Artwork height"), &mut f.art_h, 0.01);
    } else {
        size_row(ui, "taper_top_d", &tr("taper.top_diameter", "Top diameter"), &mut f.top_d, 0.01);
        size_row(ui, "taper_bottom_d", &tr("taper.bottom_diameter", "Bottom diameter"), &mut f.bottom_d, 0.01);
        size_row(ui, "taper_object_h", &tr("taper.object_height", "Object height"), &mut f.object_h, 0.01);
        ui_kit::checkbox(ui, &tr("taper.centred", "Centre on the side"), &mut f.centred);
        if !f.centred {
            size_row(ui, "taper_from_top", &tr("taper.from_top", "Down from the top"), &mut f.from_top, 0.0);
        }
    }
    let info = match result {
        Ok(p) if p.identity => tr("taper.unchanged", "These sizes leave the artwork as it is."),
        Ok(p) => tr("taper.result", "Top edge {top} mm, bottom edge {bottom} mm, height {height} mm.")
            .replace("{top}", &format!("{:.2}", p.taper.top))
            .replace("{bottom}", &format!("{:.2}", p.taper.bottom))
            .replace("{height}", &format!("{:.2}", p.taper.height)),
        Err(m) => m.clone(),
    };
    ui.add(egui::Label::new(info).wrap());
    if let Some(m) = &f.message {
        ui.add(egui::Label::new(m.as_str()).wrap());
    }
}

/// The preview on the canvas: the warped outlines drawn over the design.
pub fn paint_overlay(ctx: &egui::Context, xf: &Xf, painter: &egui::Painter, accent: Color32) {
    if !form(ctx).open {
        return;
    }
    let Some(result) = current_preview(ctx) else { return };
    let Ok(p) = result.as_ref() else { return };
    let stroke = Stroke::new(1.5, accent);
    for path in &p.paths {
        for (mut pts, closed) in render::screen_polylines(path, xf, true) {
            if closed && let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;
    use darkburn_core::kurbo::Point;

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn add_rect(a: &mut DarkBurn, x: f64, y: f64, w: f64, h: f64) -> ShapeId {
        let out = run(a, "shape.add_rect", json!({ "x": x, "y": y, "width": w, "height": h, "layer": 0 })).unwrap();
        ShapeId(out["id"].as_u64().unwrap_or_else(|| out["ids"][0].as_u64().unwrap()))
    }

    fn select(a: &mut DarkBurn, ids: &[ShapeId]) {
        run(a, "selection.set", json!({ "ids": ids.iter().map(|i| i.0).collect::<Vec<_>>() })).unwrap();
    }

    fn points(a: &DarkBurn, id: ShapeId) -> Vec<Point> {
        a.editor.doc.get(id).unwrap().outline().elements().iter().filter_map(|e| e.end_point()).collect()
    }

    fn has(pts: &[Point], p: Point) -> bool {
        pts.iter().any(|q| q.distance(p) < 1e-6)
    }

    #[test]
    fn widths_squeeze_a_square_into_a_trapezium_in_one_undo_step() {
        let mut a = app();
        let id = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0);
        select(&mut a, &[id]);
        let steps = a.editor.history.len();
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 40, "bottom": 20 })).unwrap();
        assert_eq!(out["changed"], true, "{out}");
        assert_eq!(a.editor.history.len(), steps + 1);
        let pts = points(&a, id);
        for p in [Point::new(20.0, 10.0), Point::new(40.0, 10.0), Point::new(50.0, 50.0), Point::new(10.0, 50.0)] {
            assert!(has(&pts, p), "{p:?} missing from {pts:?}");
        }
        assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Path { .. }));
        run(&mut a, "edit.undo", json!({})).unwrap();
        assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Rect { .. }), "undo brings the rectangle back");
    }

    #[test]
    fn cone_mode_bends_rows_round_the_apex() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 20.0, 10.0);
        select(&mut a, &[id]);
        run(&mut a, ID, json!({ "sizes": "widths", "top": 20, "bottom": 10, "height": 10, "mode": "cone" })).unwrap();
        // Narrow edge 10 and wide edge 20, 10 apart: the apex is at (10, -10).
        let apex = Point::new(10.0, -10.0);
        let pts = points(&a, id);
        let radii: Vec<f64> = pts.iter().map(|p| p.distance(apex)).collect();
        assert!(radii.iter().all(|r| *r > 10.0 - 1e-6 && *r < 20.0 + 1e-6), "{radii:?}");
        assert!(radii.iter().filter(|r| (**r - 20.0).abs() < 1e-6).count() > 5, "the top edge is an arc of radius 20");
        assert!(radii.iter().filter(|r| (**r - 10.0).abs() < 1e-6).count() > 5, "the bottom edge is an arc of radius 10");
    }

    #[test]
    fn diameters_keep_the_wide_end_and_shrink_the_narrow_one() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        select(&mut a, &[id]);
        let out = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "from_top": 0 })).unwrap();
        let side = 100.0f64.hypot(10.0);
        let narrow = 40.0 * (80.0 - 20.0 * 20.0 / side) / 80.0;
        assert!((out["sizes"]["top"].as_f64().unwrap() - 40.0).abs() < 1e-9);
        assert!((out["sizes"]["bottom"].as_f64().unwrap() - narrow).abs() < 1e-9, "{out}");
        let b = a.editor.doc.get(id).unwrap().bbox();
        assert!((b.width() - 40.0).abs() < 1e-6 && (b.height() - 20.0).abs() < 1e-6, "{b:?}");
        let pts = points(&a, id);
        assert!(has(&pts, Point::new(20.0 - narrow / 2.0, 0.0)), "{pts:?}");
    }

    #[test]
    fn equal_diameters_change_nothing_and_make_no_undo_step() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        select(&mut a, &[id]);
        let steps = a.editor.history.len();
        let out = run(&mut a, ID, json!({ "top": 70, "bottom": 70, "height": 90, "mode": "cone" })).unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(a.editor.history.len(), steps);
        assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Rect { .. }));
    }

    #[test]
    fn bad_sizes_and_missing_selection_are_refused_in_plain_words() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        select(&mut a, &[]);
        assert_eq!(run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100 })).unwrap_err(), "Select the shapes to warp.");
        select(&mut a, &[id]);
        let steps = a.editor.history.len();
        for args in [
            json!({ "top": 0, "bottom": 60, "height": 100 }),
            json!({ "top": 80, "bottom": -5, "height": 100 }),
            json!({ "sizes": "widths", "top": 40, "bottom": 30, "height": 0 }),
        ] {
            assert_eq!(run(&mut a, ID, args).unwrap_err(), "Every size must be greater than zero.");
        }
        assert_eq!(
            run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 10 })).unwrap_err(),
            "The artwork does not fit on the side of the object there."
        );
        assert!(run(&mut a, ID, json!({ "top": 80, "bottom": 60 })).unwrap_err().contains("height"));
        assert!(run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "mode": "twist" })).is_err());
        assert_eq!(a.editor.history.len(), steps, "nothing was changed");
    }

    #[test]
    fn images_are_refused() {
        use darkburn_core::{ImageData, ImageShape};
        let mut a = app();
        let rect = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        let mut img = None;
        a.editor.edit("Add", |d, _| {
            let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(&b"pixels"[..]), px_width: 2, px_height: 1 });
            img = Some(d.add(Shape::new(ShapeKind::Image(ImageShape { image: key, ..Default::default() }), Affine::IDENTITY, 0)));
            true
        });
        select(&mut a, &[rect, img.unwrap()]);
        let err = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100 })).unwrap_err();
        assert!(err.contains("Images cannot be taper warped"), "{err}");
        assert!(matches!(a.editor.doc.get(rect).unwrap().kind, ShapeKind::Rect { .. }));
    }

    #[test]
    fn text_becomes_a_path() {
        let mut a = app();
        let out = run(&mut a, "text.add", json!({ "text": "Cup", "x": 10, "y": 10, "height": 10, "font": "IBM Plex Sans", "layer": 0 })).unwrap();
        let id = ShapeId(out["id"].as_u64().unwrap());
        select(&mut a, &[id]);
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 30, "bottom": 20, "mode": "cone" })).unwrap();
        assert_eq!(out["changed"], true, "{out}");
        let s = a.editor.doc.get(id).unwrap();
        assert!(matches!(s.kind, ShapeKind::Path { .. }));
        assert!(darkburn_geom::path::contours(&s.outline()).len() >= 3, "the letters are still there");
    }

    #[test]
    fn a_moved_group_is_warped_where_it_is() {
        let mut a = app();
        let r1 = add_rect(&mut a, 0.0, 0.0, 10.0, 10.0);
        let r2 = add_rect(&mut a, 20.0, 0.0, 10.0, 10.0);
        select(&mut a, &[r1, r2]);
        run(&mut a, "arrange.group", json!({})).unwrap();
        let group = a.editor.selected()[0];
        a.editor.edit("Move", |d, _| {
            d.get_mut(group).unwrap().transform = Affine::translate((100.0, 50.0));
            true
        });
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 30, "bottom": 15 })).unwrap();
        assert_eq!(out["warped"], 2, "{out}");
        let s = a.editor.doc.get(group).unwrap();
        let leaves = s.leaves();
        let pts: Vec<Point> = leaves.iter().flat_map(|l| l.outline().elements().iter().filter_map(|e| e.end_point()).collect::<Vec<_>>()).collect();
        // The bottom row (y = 50) is squeezed to half width about x = 115; the top row keeps it.
        for p in [Point::new(107.5, 50.0), Point::new(122.5, 50.0), Point::new(100.0, 60.0), Point::new(130.0, 60.0)] {
            assert!(has(&pts, p), "{p:?} missing from {pts:?}");
        }
    }

    #[test]
    fn preview_reports_the_sizes_without_changing_anything() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        select(&mut a, &[id]);
        let steps = a.editor.history.len();
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 40, "bottom": 30, "preview": true })).unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(out["sizes"]["bottom"], 30.0);
        assert_eq!(out["sizes"]["height"], 20.0);
        assert_eq!(a.editor.history.len(), steps);
    }

    #[test]
    fn cone_mode_refuses_artwork_wider_than_the_cup_is_round() {
        let mut a = app();
        // A cup 80 across the top is 251 mm round; its unrolled sector is far less than a full
        // turn, so only the object's own size shows that this artwork would overlap itself.
        for width in [2500.0, 300.0] {
            let id = add_rect(&mut a, 0.0, 0.0, width, 20.0);
            select(&mut a, &[id]);
            let steps = a.editor.history.len();
            for mode in ["cone", "simple"] {
                let err = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "from_top": 0, "mode": mode })).unwrap_err();
                assert_eq!(err, "The artwork would go more than once round the object.", "{width} mm, {mode}");
            }
            assert_eq!(a.editor.history.len(), steps);
            assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Rect { .. }));
        }
        let id = add_rect(&mut a, 0.0, 0.0, 250.0, 20.0);
        select(&mut a, &[id]);
        let out = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "from_top": 0, "mode": "cone" })).unwrap();
        assert_eq!(out["changed"], true, "250 mm goes just once round: {out}");
    }

    #[test]
    fn without_from_top_the_artwork_sits_centred_on_the_side() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 20.0);
        select(&mut a, &[id]);
        let out = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "mode": "cone" })).unwrap();
        let side = 100.0f64.hypot(10.0);
        let d = |s: f64| 80.0 - 20.0 * s / side;
        let s0 = (side - 20.0) / 2.0;
        assert!((out["sizes"]["top"].as_f64().unwrap() - 40.0).abs() < 1e-9, "{out}");
        assert!((out["sizes"]["bottom"].as_f64().unwrap() - 40.0 * d(s0 + 20.0) / d(s0)).abs() < 1e-9, "{out}");
        // Placed at the top instead, the narrow edge is wider, as the cup narrows downwards.
        run(&mut a, "edit.undo", json!({})).unwrap();
        let top = run(&mut a, ID, json!({ "top": 80, "bottom": 60, "height": 100, "from_top": 0, "preview": true })).unwrap();
        assert!(top["sizes"]["bottom"].as_f64().unwrap() > out["sizes"]["bottom"].as_f64().unwrap());
    }

    #[test]
    fn locked_shapes_in_the_selection_are_left_alone() {
        let mut a = app();
        let free = add_rect(&mut a, 0.0, 0.0, 40.0, 40.0);
        let locked = add_rect(&mut a, 100.0, 0.0, 40.0, 80.0);
        a.editor.edit("Lock", |d, _| {
            d.get_mut(locked).unwrap().locked = true;
            true
        });
        select(&mut a, &[free, locked]);
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 40, "bottom": 20 })).unwrap();
        assert_eq!(out["warped"], 1, "{out}");
        assert!(matches!(a.editor.doc.get(locked).unwrap().kind, ShapeKind::Rect { .. }), "the locked shape is untouched");
        // The box is the free shape's alone: its bottom edge is squeezed about x = 20.
        let pts = points(&a, free);
        for p in [Point::new(10.0, 0.0), Point::new(30.0, 0.0), Point::new(40.0, 40.0), Point::new(0.0, 40.0)] {
            assert!(has(&pts, p), "{p:?} missing from {pts:?}");
        }
        select(&mut a, &[locked]);
        assert_eq!(run(&mut a, ID, json!({ "sizes": "widths", "top": 40, "bottom": 20 })).unwrap_err(), "Select the shapes to warp.");
    }

    #[test]
    fn sizes_beyond_the_windows_limit_are_refused() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 30.0, 30.0);
        select(&mut a, &[id]);
        let steps = a.editor.history.len();
        for args in [
            json!({ "sizes": "widths", "top": 1e8, "bottom": 5e7, "height": 1e8, "mode": "cone" }),
            json!({ "sizes": "widths", "top": 1e8, "bottom": 5e7, "height": 1e8, "mode": "cone", "preview": true }),
            json!({ "sizes": "widths", "top": 40, "bottom": 30, "height": 10_000.5 }),
            json!({ "top": 20_000, "bottom": 60, "height": 100 }),
            json!({ "top": 80, "bottom": 60, "height": 100, "from_top": 1e6 }),
        ] {
            let err = run(&mut a, ID, args.clone()).unwrap_err();
            assert!(err.contains("at most 10000 mm"), "{args}: {err}");
        }
        assert_eq!(a.editor.history.len(), steps);
        assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Rect { .. }));
        // The limit itself is fine.
        let out = run(&mut a, ID, json!({ "sizes": "widths", "top": 10_000, "bottom": 5_000, "height": 10_000, "mode": "cone" })).unwrap();
        assert_eq!(out["changed"], true, "{out}");
    }

    #[test]
    fn the_window_opens_with_the_selection_and_previews_it() {
        let mut a = app();
        let id = add_rect(&mut a, 0.0, 0.0, 40.0, 120.0);
        select(&mut a, &[id]);
        let ctx = egui::Context::default();
        assert!(super::run(&mut a, &ctx, ID));
        let f = form(&ctx);
        assert!(f.open);
        assert_eq!((f.top_w, f.bottom_w, f.art_h), (40.0, 40.0, 120.0));
        assert!(f.object_h > 120.0, "the object was made tall enough for the artwork");
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(ui.ctx(), &mut a));
        out.textures_delta.clear();
        let p = current_preview(&ctx).expect("a preview");
        let p = p.as_ref().as_ref().unwrap();
        assert_eq!(p.paths.len(), 1);
        assert!((p.taper.top - 40.0).abs() < 1e-9 && p.taper.bottom < 40.0);
        assert!(matches!(a.editor.doc.get(id).unwrap().kind, ShapeKind::Rect { .. }), "the preview changes nothing");
    }
}
