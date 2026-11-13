//! 3D models (STL, OBJ) become grey height pictures for 3D engraving [IMG-25].
//!
//! Import and drag and drop hand a model file to the Import 3D Model window instead of the
//! artwork reader. The window reads the model on a worker, shows its size, and draws a preview
//! of the height picture as the view, the width, the tones and the depth range change. OK
//! draws the full picture on a worker and puts it in as an ordinary image shape (a 16-bit grey
//! PNG) on an image layer, as one undo step.
//!
//! `image.depth_map` is the headless form: a model file and the settings in, the image shape
//! out.
//!
//! The image layer: the one asked for, else the current layer when it is an image layer, else
//! the current layer when it is empty, else the first empty layer that is not a tool layer,
//! else the current layer. An empty layer used this way is set to Image mode with Grayscale, the
//! mode that turns grey into power for 3D engraving; a layer with shapes on it keeps its mode.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use darkburn_core::kurbo::{Affine, Point};
use darkburn_core::{Document, Editor, ImageData, ImageMode, ImageShape, LAYER_COUNT, Mode, Shape, ShapeId, ShapeKind};
use darkburn_import::depth_map::{self as depth, DepthLimits, DepthMap, DepthSettings, View};
use darkburn_import::mesh::{self, Mesh, MeshLimits};
use darkburn_ui::ui_kit;
use egui::{Align2, Color32, Context, Id, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2};
use serde_json::{Value, json};

use crate::i18n::tr;
use crate::image_ui;
use crate::numeric::{self, Num};
use crate::shell::DarkBurn;

/// The undo step a model import makes.
pub const UNDO_LABEL: &str = "Import 3D model";
/// Resolution of a new picture, dots per inch.
pub const DEFAULT_DPI: f64 = 254.0;
/// Width used when the model's own width (taken as millimetres) is not a sensible picture width.
pub const FALLBACK_WIDTH_MM: f64 = 100.0;
/// Widest picture asked for, mm.
pub const MAX_WIDTH_MM: f64 = 5000.0;
/// Model files waiting for the window while it shows another.
const MAX_QUEUE: usize = 50;
/// Pixels on the longer side of the preview.
const PREVIEW_SIDE: f64 = 300.0;

/// Whether `path` names a 3D model file Import reads into a height picture.
pub fn is_model(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).and_then(mesh::format_of).is_some()
}

/// Reads model `path` under the import limits (file size, triangle budget).
pub fn read_model(path: &Path) -> Result<Mesh, String> {
    let format = path.extension().and_then(|e| e.to_str()).and_then(mesh::format_of).ok_or("is not an STL or OBJ file")?;
    let meta = std::fs::metadata(path).map_err(|e| format!("could not be read ({e})"))?;
    if !meta.is_file() {
        return Err("is not a file".into());
    }
    if meta.len() > crate::formats::MAX_FILE_BYTES {
        return Err(format!("is larger than {} MB", crate::formats::MAX_FILE_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("could not be read ({e})"))?;
    std::panic::catch_unwind(AssertUnwindSafe(|| mesh::read_mesh(format, &bytes, &MeshLimits::default())))
        .unwrap_or_else(|_| Err(darkburn_import::ImportError::Parse("the model could not be read".into())))
        .map_err(|e| e.to_string())
}

/// The picture width a model starts with: its own width seen from `view` when that is a
/// sensible number of millimetres, else [`FALLBACK_WIDTH_MM`].
pub fn natural_width(m: &Mesh, view: View, flip: bool) -> f64 {
    let w = depth::extent(m, view, flip)[0];
    if (1.0..=2000.0).contains(&w) { (w * 100.0).round() / 100.0 } else { FALLBACK_WIDTH_MM }
}

/// A finished height picture, ready to go into the document.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub png: Vec<u8>,
    pub px: (u32, u32),
    pub size_mm: (f64, f64),
    /// Visible depth of the scaled model, mm.
    pub depth_mm: f64,
}

/// Draws the height picture of `m`.
pub fn render(m: &Mesh, s: &DepthSettings) -> Result<Rendered, String> {
    let map = std::panic::catch_unwind(AssertUnwindSafe(|| depth::depth_map(m, s, &DepthLimits::default())))
        .unwrap_or_else(|_| Err(darkburn_import::ImportError::Parse("the picture could not be drawn".into())))
        .map_err(|e| e.to_string())?;
    let png = map.to_png().map_err(|e| e.to_string())?;
    let f = map.frame;
    Ok(Rendered { png, px: (f.px_width, f.px_height), size_mm: (f.width_mm, f.height_mm), depth_mm: map.visible_depth_mm })
}

/// The layer a model's picture goes on, and whether that layer is to be made a grey image layer
/// first (see the module notes).
pub fn model_layer(doc: &Document, wanted: Option<usize>, current: usize) -> (usize, bool) {
    let empty = |l: usize| doc.shapes_on_layer(l).is_empty();
    let is_image = |l: usize| doc.layer(l).main().mode == Mode::Image;
    let prepare = |l: usize| (l, empty(l) && !is_image(l));
    let usable = |l: usize| l < LAYER_COUNT && !darkburn_core::layer::is_tool_layer(l);
    if let Some(l) = wanted.filter(|l| usable(*l)) {
        return prepare(l);
    }
    if usable(current) && is_image(current) {
        return (current, false);
    }
    if usable(current) && empty(current) {
        return prepare(current);
    }
    match (0..LAYER_COUNT).find(|l| usable(*l) && empty(*l)) {
        Some(l) => prepare(l),
        None => (current, false),
    }
}

/// Puts `r` into the document as an image shape centred on `centre`, as one undo step, and
/// selects it. Returns the shape and its layer.
pub fn insert(e: &mut Editor, r: &Rendered, wanted_layer: Option<usize>, centre: Point) -> Result<(ShapeId, usize), String> {
    let (layer, prepare) = model_layer(&e.doc, wanted_layer, e.current_layer);
    let mut made = None;
    let done = e.edit(UNDO_LABEL, |d, sel| {
        if prepare {
            let c = d.layer_mut(layer).main_mut();
            c.mode = Mode::Image;
            c.image_mode = ImageMode::Grayscale;
        }
        let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(r.png.as_slice()), px_width: r.px.0, px_height: r.px.1 });
        let image = ImageShape { image: key, width: r.size_mm.0, height: r.size_mm.1, ..Default::default() };
        let id = d.add(Shape::new(ShapeKind::Image(image), Affine::translate(centre.to_vec2()), layer));
        sel.set([id]);
        made = Some(id);
        true
    });
    match made {
        Some(id) if done => Ok((id, layer)),
        _ => Err("the picture would lie too far outside the work area".into()),
    }
}

fn page_centre(app: &DarkBurn) -> Point {
    Point::new(f64::from(app.prefs.page_width) / 2.0, f64::from(app.prefs.page_height) / 2.0)
}

fn opt_num(a: &Value, key: &str) -> Result<Option<f64>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|v| v.is_finite()).map(Some).ok_or_else(|| format!("argument {key} must be a number")),
    }
}

fn opt_bool(a: &Value, key: &str) -> Result<Option<bool>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| format!("argument {key} must be true or false")),
    }
}

/// The arguments of `image.depth_map`, checked before the model is read.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub path: PathBuf,
    pub layer: Option<usize>,
    pub centre: Point,
    /// The settings; the width is filled in from the model when the arguments leave it out.
    pub settings: DepthSettings,
    pub width: Option<f64>,
}

impl Request {
    /// Checks the arguments of `image.depth_map`. Nothing is read yet.
    pub fn parse(app: &DarkBurn, a: &Value) -> Result<Request, String> {
        let path = a.get("path").and_then(Value::as_str).ok_or("argument path must be a file path")?;
        let layer = match a.get("layer") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|l| *l < LAYER_COUNT as u64 && !darkburn_core::layer::is_tool_layer(*l as usize))
                    .ok_or(format!("argument layer must be 0 to {}", darkburn_core::T1 - 1))? as usize,
            ),
        };
        let centre = match (opt_num(a, "x")?, opt_num(a, "y")?) {
            (Some(x), Some(y)) => Point::new(x, y),
            (None, None) => page_centre(app),
            _ => return Err("give both x and y, or neither".into()),
        };
        let view = match a.get("view") {
            None | Some(Value::Null) => View::Top,
            Some(v) => v.as_str().and_then(View::from_name).ok_or("argument view must be top, front or side")?,
        };
        let width = opt_num(a, "width")?;
        if width.is_some_and(|w| !(w > 0.0 && w <= MAX_WIDTH_MM)) {
            return Err(format!("argument width must be more than 0 and at most {MAX_WIDTH_MM} mm"));
        }
        let dpi = opt_num(a, "dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=2400.0).contains(&dpi) {
            return Err("argument dpi must be 1 to 2400".into());
        }
        let (depth_from, depth_to) = (opt_num(a, "depth_from")?, opt_num(a, "depth_to")?);
        if [depth_from, depth_to].into_iter().flatten().any(|v| v < 0.0) {
            return Err("argument depth_from and depth_to must be 0 or more".into());
        }
        if let (Some(from), Some(to)) = (depth_from, depth_to)
            && to <= from
        {
            return Err("argument depth_to must be deeper than depth_from".into());
        }
        let settings = DepthSettings {
            view,
            flip: opt_bool(a, "flip")?.unwrap_or(false),
            width_mm: width.unwrap_or(FALLBACK_WIDTH_MM),
            dpi,
            near_white: opt_bool(a, "near_white")?.unwrap_or(true),
            depth_from,
            depth_to,
        };
        Ok(Request { path: PathBuf::from(path), layer, centre, settings, width })
    }

    /// The settings for model `m`: the model's own width when none was asked for.
    pub fn settings_for(&self, m: &Mesh) -> DepthSettings {
        let width_mm = self.width.unwrap_or_else(|| natural_width(m, self.settings.view, self.settings.flip));
        DepthSettings { width_mm, ..self.settings }
    }

    /// Reads the model and draws its picture: the slow part, which needs no app. Returns the
    /// picture and the model's triangle count.
    pub fn make(&self) -> Result<(Rendered, usize), String> {
        let m = read_model(&self.path).map_err(|e| format!("{}: {e}", crate::multi_trace::name_of(&self.path)))?;
        let r = render(&m, &self.settings_for(&m))?;
        Ok((r, m.triangles.len()))
    }

    /// Puts a finished picture into the document and reports it.
    pub fn land(&self, e: &mut Editor, made: Result<(Rendered, usize), String>) -> Result<Value, String> {
        let (r, triangles) = made?;
        let (id, layer) = insert(e, &r, self.layer, self.centre)?;
        Ok(json!({
            "created": id.0,
            "layer": layer,
            "px": [r.px.0, r.px.1],
            "size_mm": [r.size_mm.0, r.size_mm.1],
            "depth_mm": r.depth_mm,
            "triangles": triangles,
        }))
    }
}

/// `image.depth_map`: reads model `path`, draws its height picture with the settings in `a`
/// and puts it in as an image shape (at `x`, `y`, else the middle of the work area). This runs
/// on the calling thread (scripts and tests); an agent's call goes through [`PendingDepthMap`].
pub fn execute(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let req = Request::parse(app, a)?;
    let made = req.make();
    req.land(&mut app.editor, made)
}

/// An `image.depth_map` call running on a worker, so a large model does not freeze the window.
/// The picture goes into the document when the agent asks for the result.
pub struct PendingDepthMap {
    req: Request,
    job: JoinHandle<Result<(Rendered, usize), String>>,
}

impl PendingDepthMap {
    /// Checks the arguments and starts reading and drawing.
    pub fn start(app: &DarkBurn, a: &Value) -> Result<PendingDepthMap, String> {
        let req = Request::parse(app, a)?;
        let worker = req.clone();
        Ok(PendingDepthMap { req, job: std::thread::spawn(move || worker.make()) })
    }

    pub fn finished(&self) -> bool {
        self.job.is_finished()
    }

    /// Puts the picture into the document, as [`execute`] does. Waits for the worker if it has
    /// not finished.
    pub fn finish(self, app: &mut DarkBurn) -> Result<Value, String> {
        let made = self.job.join().unwrap_or_else(|_| Err("the picture could not be drawn".into()));
        self.req.land(&mut app.editor, made)
    }
}

// ----- Window -----

/// Model files waiting for the window, and the window.
#[derive(Default)]
pub struct DepthUi {
    pub dialog: Option<DepthDialog>,
    pub queue: Vec<(PathBuf, Option<Point>)>,
}

/// The Import 3D Model window.
pub struct DepthDialog {
    pub path: PathBuf,
    /// Where a drop put the model, else the middle of the work area.
    pub at: Option<Point>,
    pub settings: DepthSettings,
    /// The depth range is limited (`from` and `to` below are used).
    pub clamp: bool,
    pub from: f64,
    pub to: f64,
    pub model: Option<Arc<Mesh>>,
    pub error: Option<String>,
    loading: Option<JoinHandle<Result<Mesh, String>>>,
    preview: Option<(DepthSettings, JoinHandle<Result<DepthMap, String>>)>,
    shown_for: Option<DepthSettings>,
    texture: Option<TextureHandle>,
    preview_size: (f64, f64),
    preview_error: Option<String>,
    job: Option<JoinHandle<Result<Rendered, String>>>,
}

impl DepthDialog {
    pub fn new(path: PathBuf, at: Option<Point>) -> DepthDialog {
        let p = path.clone();
        DepthDialog {
            path,
            at,
            settings: DepthSettings { dpi: DEFAULT_DPI, ..DepthSettings::default() },
            clamp: false,
            from: 0.0,
            to: 5.0,
            model: None,
            error: None,
            loading: Some(std::thread::spawn(move || read_model(&p))),
            preview: None,
            shown_for: None,
            texture: None,
            preview_size: (100.0, 100.0),
            preview_error: None,
            job: None,
        }
    }

    /// The settings with the depth range applied.
    pub fn effective(&self) -> DepthSettings {
        let (depth_from, depth_to) = if self.clamp { (Some(self.from), Some(self.to)) } else { (None, None) };
        DepthSettings { depth_from, depth_to, ..self.settings }
    }

    /// The preview shows the current settings.
    #[cfg(test)]
    pub fn preview_ready(&self) -> bool {
        self.texture.is_some() && self.shown_for.is_some_and(|s| same_picture(&s, &self.effective()))
    }

    fn take_model(&mut self) {
        if self.loading.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(h) = self.loading.take()
        {
            match h.join().unwrap_or_else(|_| Err("the model could not be read".into())) {
                Ok(m) => {
                    self.settings.width_mm = natural_width(&m, self.settings.view, self.settings.flip);
                    self.model = Some(Arc::new(m));
                }
                Err(e) => self.error = Some(format!("{}: {e}", crate::multi_trace::name_of(&self.path))),
            }
        }
    }

    /// Takes a finished preview and starts the next one when the settings changed. One runs at
    /// a time.
    fn refresh(&mut self, ctx: &Context) {
        self.take_model();
        if self.preview.as_ref().is_some_and(|(_, h)| h.is_finished())
            && let Some((s, h)) = self.preview.take()
        {
            match h.join().unwrap_or_else(|_| Err("the preview could not be drawn".into())) {
                Ok(map) => {
                    let grey: Vec<u8> = map.tones.iter().map(|v| (v >> 8) as u8).collect();
                    let image = egui::ColorImage::from_gray([map.frame.px_width as usize, map.frame.px_height as usize], &grey);
                    self.texture = Some(ctx.load_texture("db_model_depth_preview", image, TextureOptions::LINEAR));
                    self.preview_size = (map.frame.width_mm, map.frame.height_mm);
                    self.preview_error = None;
                }
                Err(e) => {
                    self.texture = None;
                    self.preview_error = Some(e);
                }
            }
            self.shown_for = Some(s);
        }
        let want = self.effective();
        let stale = self.shown_for.is_none_or(|s| !same_picture(&s, &want));
        if stale
            && self.preview.is_none()
            && let Some(m) = self.model.clone()
        {
            let small = preview_settings(&m, &want);
            self.preview = Some((want, std::thread::spawn(move || depth::depth_map(&m, &small, &DepthLimits::default()).map_err(|e| e.to_string()))));
        }
        if self.loading.is_some() || self.preview.is_some() || self.job.is_some() {
            ctx.request_repaint();
        }
    }

    /// The picture size the settings give, for the caption.
    fn frame(&self) -> Option<Result<depth::Frame, String>> {
        let m = self.model.as_ref()?;
        Some(depth::frame(m, &self.effective(), &DepthLimits::default()).map_err(|e| e.to_string()))
    }
}

/// Two settings give the same picture apart from its resolution.
fn same_picture(a: &DepthSettings, b: &DepthSettings) -> bool {
    DepthSettings { dpi: 1.0, ..*a } == DepthSettings { dpi: 1.0, ..*b }
}

/// `s` at a resolution that makes the preview about [`PREVIEW_SIDE`] pixels on its longer side.
fn preview_settings(m: &Mesh, s: &DepthSettings) -> DepthSettings {
    let [w, h, _] = depth::extent(m, s.view, s.flip);
    let height_mm = if w > 0.0 { h / w * s.width_mm } else { s.width_mm };
    let longer = s.width_mm.max(height_mm).max(1e-6);
    DepthSettings { dpi: PREVIEW_SIDE / longer * 25.4, ..*s }
}

/// Model files from Import or a drop: the first opens the window, the rest wait their turn.
pub fn open_files(app: &mut DarkBurn, paths: Vec<PathBuf>, at: Option<Point>) {
    for p in paths {
        if app.images.depth.dialog.is_none() {
            app.images.depth.dialog = Some(DepthDialog::new(p, at));
        } else if app.images.depth.queue.len() < MAX_QUEUE {
            app.images.depth.queue.push((p, at));
        }
    }
}

/// The View choices: the three views, then the same three seen from the other side.
fn views() -> [String; 6] {
    [
        tr("model_import.top", "Top"),
        tr("model_import.front", "Front"),
        tr("model_import.side", "Side"),
        tr("model_import.bottom", "Bottom"),
        tr("model_import.back", "Back"),
        tr("model_import.left_side", "Left side"),
    ]
}

fn view_index(s: &DepthSettings) -> usize {
    View::ALL.iter().position(|v| *v == s.view).unwrap_or(0) + if s.flip { 3 } else { 0 }
}

/// A dim line of facts, wrapped to the column.
fn note(ui: &mut Ui, text: &str) {
    ui_kit::status_line(ui, text, ui_kit::StatusTone::Dim);
}

fn body(ui: &mut Ui, d: &mut DepthDialog) {
    let mut rule = 0.0;
    let row = ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(320.0);
            ui_kit::section_heading(ui, &tr("model_import.model", "Model"));
            note(ui, &crate::multi_trace::name_of(&d.path));
            match (&d.model, &d.error) {
                (_, Some(e)) => {
                    ui_kit::status_line(ui, e, ui_kit::StatusTone::Error);
                }
                (Some(m), None) => {
                    let [x, y, z] = m.size();
                    let size = tr("model_import.size", "Size X {x}, Y {y}, Z {z}; {n} triangles")
                        .replace("{x}", &format!("{x:.2}"))
                        .replace("{y}", &format!("{y:.2}"))
                        .replace("{z}", &format!("{z:.2}"))
                        .replace("{n}", &m.triangles.len().to_string());
                    note(ui, &size);
                }
                (None, None) => note(ui, &tr("model_import.reading", "Reading the model...")),
            }
            ui.add_enabled_ui(d.model.is_some() && d.job.is_none(), |ui| {
                ui_kit::section_heading(ui, &tr("model_import.view", "View"));
                let mut view = view_index(&d.settings);
                let views = views();
                let names: Vec<&str> = views.iter().map(String::as_str).collect();
                ui_kit::choice_row(ui, &tr("model_import.look_from", "Look from"), "model_depth_view", &mut view, &names);
                if view != view_index(&d.settings) {
                    d.settings.view = View::ALL[view % 3];
                    d.settings.flip = view >= 3;
                }
                ui_kit::section_heading(ui, &tr("model_import.picture", "Picture"));
                numeric::row(ui, &tr("model_import.width", "Width"), &mut d.settings.width_mm, &Num::new(1.0..=MAX_WIDTH_MM, 2, " mm").speed(0.1));
                numeric::row(ui, &tr("model_import.resolution", "Resolution"), &mut d.settings.dpi, &Num::new(10.0..=1200.0, 0, " DPI").speed(1.0));
                let mut near = usize::from(!d.settings.near_white);
                let tones = [tr("model_import.white", "White"), tr("model_import.black", "Black")];
                ui_kit::choice_row(ui, &tr("model_import.near", "Near surfaces"), "model_depth_near", &mut near, &[&tones[0], &tones[1]]);
                d.settings.near_white = near == 0;
                ui_kit::checkbox(ui, &tr("model_import.limit_depth", "Limit the depth range"), &mut d.clamp);
                if d.clamp {
                    numeric::row(ui, &tr("model_import.from_depth", "From depth"), &mut d.from, &Num::new(0.0..=1000.0, 2, " mm").speed(0.05));
                    numeric::row(ui, &tr("model_import.to_depth", "To depth"), &mut d.to, &Num::new(0.0..=1000.0, 2, " mm").speed(0.05));
                }
            });
            match d.frame() {
                Some(Ok(f)) => {
                    let deep = f.model_depth * f.scale;
                    let facts = tr("model_import.facts", "Picture {w} x {h} mm, {pw} x {ph} pixels, {d} mm deep")
                        .replace("{w}", &format!("{:.2}", f.width_mm))
                        .replace("{h}", &format!("{:.2}", f.height_mm))
                        .replace("{pw}", &f.px_width.to_string())
                        .replace("{ph}", &f.px_height.to_string())
                        .replace("{d}", &format!("{deep:.2}"));
                    note(ui, &facts);
                }
                Some(Err(e)) => {
                    ui_kit::status_line(ui, &e, ui_kit::StatusTone::Warning);
                }
                None => {}
            }
            if d.job.is_some() {
                note(ui, &tr("model_import.making", "Making the picture..."));
            }
        });
        rule = image_ui::column_gap(ui);
        ui.vertical(|ui| preview(ui, d));
    });
    image_ui::column_rule(ui, rule, row.response.rect);
}

fn preview(ui: &mut Ui, d: &DepthDialog) {
    let side = 300.0f32;
    let (w, h) = d.preview_size;
    let aspect = (w / h.max(1e-9)) as f32;
    let size = if aspect >= 1.0 { vec2(side, (side / aspect).max(1.0)) } else { vec2((side * aspect).max(1.0), side) };
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, 0.0, Color32::from_gray(40));
    if let Some(tex) = &d.texture {
        p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    if let Some(e) = &d.preview_error {
        ui_kit::status_line(ui, e, ui_kit::StatusTone::Warning);
    }
}

/// Draws the Import 3D Model window; called once a frame from the image windows.
pub fn dialog(ctx: &Context, app: &mut DarkBurn) {
    if app.images.depth.dialog.is_none() && !app.images.depth.queue.is_empty() {
        let (p, at) = app.images.depth.queue.remove(0);
        app.images.depth.dialog = Some(DepthDialog::new(p, at));
    }
    let Some(mut d) = app.images.depth.dialog.take() else { return };
    d.refresh(ctx);
    let valid = d.model.is_some() && d.job.is_none() && matches!(d.frame(), Some(Ok(_)));
    let (title, ok, cancel) = (tr("model_import.title", "Import 3D Model"), tr("model_import.import", "Import"), tr("model_import.cancel", "Cancel"));
    let (closed, action) = ui_kit::Window::new(Id::unique("db_model_depth"), "import", &title)
        .cards(2.9)
        .fields()
        .placed(Align2::CENTER_CENTER, crate::shell::room_under_the_bar(ctx).center())
        .actions(&ok, &cancel, None)
        .show(ctx, valid, |ui| image_ui::under_the_bar(ui, |ui| body(ui, &mut d)));
    if closed || action == ui_kit::Action::Cancel {
        return;
    }
    if action == ui_kit::Action::Ok
        && valid
        && let Some(m) = d.model.clone()
    {
        let s = d.effective();
        d.job = Some(std::thread::spawn(move || render(&m, &s)));
        ctx.request_repaint();
    }
    if d.job.as_ref().is_some_and(JoinHandle::is_finished)
        && let Some(job) = d.job.take()
    {
        let centre = d.at.unwrap_or_else(|| page_centre(app));
        match job.join().unwrap_or_else(|_| Err("the picture could not be drawn".into())).and_then(|r| insert(&mut app.editor, &r, None, centre)) {
            Ok(_) => {
                app.note =
                    Some(tr("model_import.imported", "Imported {name} as a height picture").replace("{name}", &crate::multi_trace::name_of(&d.path)));
                return;
            }
            Err(e) => d.error = Some(e),
        }
    }
    app.images.depth.dialog = Some(d);
}

/// The window with a small model, for the window sweeps (fit, Escape, names), which cannot open
/// it through the file picker.
#[cfg(test)]
pub fn state_case_dialog() -> DepthDialog {
    let dir = std::env::temp_dir().join(format!("darkburn-model-case-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{:?}.stl", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric() && c != '.', "-"));
    if !p.exists() {
        let _ = std::fs::write(&p, mesh::write_binary_stl(&tests::pyramid()));
    }
    let mut d = DepthDialog::new(p, None);
    // Wait for the small model, so the window shows its settings.
    let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while d.loading.as_ref().is_some_and(|h| !h.is_finished()) && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    d.take_model();
    d
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::prefs::Prefs;
    use darkburn_core::kurbo::Shape as _;

    /// A square pyramid: base 0..20 by 0..20 at z = 0, apex (10, 10, 10).
    pub fn pyramid() -> Mesh {
        let (a, b, c, d, apex) = ([0.0, 0.0, 0.0], [20.0, 0.0, 0.0], [20.0, 20.0, 0.0], [0.0, 20.0, 0.0], [10.0, 10.0, 10.0]);
        Mesh { triangles: vec![[a, b, apex], [b, c, apex], [c, d, apex], [d, a, apex], [a, c, b], [a, d, c]] }
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkburn-model-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn image_of(app: &DarkBurn, id: u64) -> (Shape, image::DynamicImage) {
        let s = app.editor.doc.get(ShapeId(id)).unwrap().clone();
        let ShapeKind::Image(img) = &s.kind else { panic!("an image shape") };
        let data = app.editor.doc.images.get(&img.image).expect("pixels stored");
        let picture = image::load_from_memory(&data.bytes).unwrap();
        (s, picture)
    }

    #[test]
    fn the_command_turns_a_model_file_into_a_grey_image_shape() {
        let dir = temp("headless");
        let stl = write(&dir, "pyramid.stl", &mesh::write_binary_stl(&pyramid()));
        let mut app = DarkBurn::new(Prefs::default());
        let v = crate::api::execute_headless(
            &mut app,
            "image.depth_map",
            &json!({ "path": stl.display().to_string(), "width": 40, "dpi": 25.4, "x": 60, "y": 70 }),
        )
        .unwrap();
        assert_eq!(v["px"], json!([40, 40]), "{v}");
        assert_eq!(v["triangles"], 6);
        assert!((v["depth_mm"].as_f64().unwrap() - 19.0).abs() < 1e-3, "{v}");
        let (shape, picture) = image_of(&app, v["created"].as_u64().unwrap());
        let ShapeKind::Image(img) = &shape.kind else { unreachable!() };
        assert!((img.width - 40.0).abs() < 1e-9 && (img.height - 40.0).abs() < 1e-9);
        assert!(img.source.is_none(), "Refresh Image cannot reread a model as a picture");
        let c = shape.bbox().center();
        assert!((c.x - 60.0).abs() < 1e-6 && (c.y - 70.0).abs() < 1e-6, "{c:?}");
        // 16-bit grey: the apex white, the corners black.
        let grey = picture.as_luma16().expect("16-bit grey");
        assert_eq!(grey.get_pixel(19, 19).0[0], 65535);
        assert_eq!(grey.get_pixel(0, 0).0[0], 0);
        // On an image layer set to grayscale, as one undo step that is selected.
        let layer = app.editor.doc.layer(shape.layer).main();
        assert_eq!((layer.mode, layer.image_mode), (Mode::Image, ImageMode::Grayscale));
        assert_eq!(app.editor.selected(), vec![shape.id]);
        assert_eq!(app.editor.history.undo_label(), Some(UNDO_LABEL));
        assert!(app.editor.undo());
        assert_eq!(app.editor.doc.len(), 0);
        assert_ne!(app.editor.doc.layer(shape.layer).main().mode, Mode::Image, "undo puts the layer back too");
    }

    #[test]
    fn the_command_takes_every_setting() {
        let dir = temp("settings");
        let obj = write(&dir, "p.obj", b"v 0 0 0\nv 20 0 0\nv 20 20 0\nv 0 20 0\nv 10 10 10\nf 1 2 5\nf 2 3 5\nf 3 4 5\nf 4 1 5\nf 1 3 2\nf 1 4 3\n");
        let mut app = DarkBurn::new(Prefs::default());
        let path = obj.display().to_string();
        // Default: the model's own width at 254 DPI, in the middle of the work area.
        let v = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path })).unwrap();
        assert_eq!(v["size_mm"], json!([20.0, 20.0]), "{v}");
        assert_eq!(v["px"], json!([200, 200]));
        let c = app.editor.doc.get(ShapeId(v["created"].as_u64().unwrap())).unwrap().bbox().center();
        assert!((c - page_centre(&app)).hypot() < 1e-6);
        // From the front: 20 wide, 10 tall; near black.
        let v = crate::api::execute_headless(
            &mut app,
            "image.depth_map",
            &json!({ "path": path, "view": "front", "width": 20, "dpi": 25.4, "near_white": false, "layer": 9 }),
        )
        .unwrap();
        assert_eq!(v["px"], json!([20, 10]), "{v}");
        assert_eq!(v["layer"], 9);
        let (_, picture) = image_of(&app, v["created"].as_u64().unwrap());
        let grey = picture.as_luma16().unwrap();
        assert_eq!(grey.get_pixel(10, 9).0[0], 0, "the nearest point is black");
        assert_eq!(grey.get_pixel(0, 0).0[0], 65535, "nothing there: the far tone");
        // A depth range: 2 to 4 mm below the top.
        let v = crate::api::execute_headless(
            &mut app,
            "image.depth_map",
            &json!({ "path": path, "width": 20, "dpi": 25.4, "depth_from": 2, "depth_to": 4 }),
        )
        .unwrap();
        let (_, picture) = image_of(&app, v["created"].as_u64().unwrap());
        let grey = picture.as_luma16().unwrap();
        assert_eq!(grey.get_pixel(8, 8).0[0], 65535);
        assert_eq!(grey.get_pixel(3, 3).0[0], 0);
        // Bad arguments.
        for bad in [
            json!({}),
            json!({ "path": path, "view": "diagonal" }),
            json!({ "path": path, "width": -1 }),
            json!({ "path": path, "dpi": 0 }),
            json!({ "path": path, "x": 3 }),
            json!({ "path": path, "layer": 99 }),
            json!({ "path": path, "layer": 30 }),
            json!({ "path": path, "depth_from": 3, "depth_to": 1 }),
            json!({ "path": path, "depth_from": -1 }),
            json!({ "path": path, "width": 5000, "dpi": 2400 }),
            json!({ "path": dir.join("missing.stl").display().to_string() }),
            json!({ "path": dir.join("p.png").display().to_string() }),
        ] {
            let before = app.editor.doc.len();
            assert!(crate::api::execute_headless(&mut app, "image.depth_map", &bad).is_err(), "{bad}");
            assert_eq!(app.editor.doc.len(), before);
        }
        let broken = write(&dir, "broken.stl", &mesh::write_binary_stl(&pyramid())[..200]);
        let e = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": broken.display().to_string() })).unwrap_err();
        assert!(e.contains("broken.stl") && e.contains("ends early"), "{e}");
    }

    #[test]
    fn the_picture_goes_on_an_image_layer() {
        let mut doc = Document::default();
        // An empty current layer is used and made a grey image layer.
        assert_eq!(model_layer(&doc, None, 3), (3, true));
        // A current image layer is used as it is.
        crate::images::testutil::image_layer(&mut doc, 5, 0.1);
        doc.add(Shape::path(darkburn_core::kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1), 5));
        assert_eq!(model_layer(&doc, None, 5), (5, false));
        // A current line layer with shapes: the first empty layer instead.
        doc.add(Shape::path(darkburn_core::kurbo::Rect::new(0.0, 0.0, 1.0, 1.0).to_path(0.1), 0));
        assert_eq!(model_layer(&doc, None, 0), (1, true));
        // A tool layer is never picked.
        let t1 = darkburn_core::T1;
        assert_ne!(model_layer(&doc, None, t1).0, t1);
        // A layer asked for is used; it keeps its mode when it holds shapes.
        assert_eq!(model_layer(&doc, Some(0), 5), (0, false));
        assert_eq!(model_layer(&doc, Some(7), 5), (7, true));
    }

    #[test]
    fn import_and_drop_hand_models_to_the_window() {
        let dir = temp("route");
        let a = write(&dir, "a.stl", &mesh::write_binary_stl(&pyramid()));
        let b = write(&dir, "b.OBJ", b"v 0 0 0\nv 1 0 0\nv 0 1 1\nf 1 2 3\n");
        let svg = write(&dir, "c.svg", br#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm"><rect width="5" height="5"/></svg>"#);
        assert!(is_model(&a) && is_model(&b) && !is_model(&svg));
        let mut app = DarkBurn::new(Prefs::default());
        crate::files::drop_files(&mut app, &Context::default(), vec![a.clone(), svg, b.clone()], Some(Point::new(30.0, 40.0)));
        let d = app.images.depth.dialog.as_ref().expect("the window opens for the first model");
        assert_eq!((d.path.as_path(), d.at), (a.as_path(), Some(Point::new(30.0, 40.0))));
        assert_eq!(app.images.depth.queue, vec![(b, Some(Point::new(30.0, 40.0)))]);
        assert!(app.fio.reading(), "the artwork goes to the usual reader");
    }

    #[test]
    fn hostile_files_are_refused_without_harm() {
        let dir = temp("hostile");
        let mut app = DarkBurn::new(Prefs::default());
        // A header claiming four billion triangles.
        let mut huge = vec![b' '; 84];
        huge[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        let huge = write(&dir, "huge.stl", &huge);
        let nan = write(&dir, "nan.obj", b"v 0 0 0\nv nan 0 0\nv 0 1 0\nf 1 2 3\n");
        let flat = write(&dir, "flat.obj", b"v 0 0 0\nv 0 1 0\nv 0 1 1\nf 1 2 3\n");
        for p in [huge, nan, flat] {
            let r = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": p.display().to_string(), "width": 10 }));
            assert!(r.is_err(), "{}", p.display());
        }
        assert_eq!(app.editor.doc.len(), 0);
        assert!(!app.editor.history.can_undo());
    }

    /// A wedge: 20 long in X, 10 deep in Y, its top sloping from 10 high at x = 0 down to 0 at
    /// x = 20. Seen from the front its outline is a right triangle with the tall side on the left.
    const WEDGE: &[u8] = b"v 0 0 0\nv 20 0 0\nv 20 10 0\nv 0 10 0\nv 0 0 10\nv 0 10 10\nf 1 2 3 4\nf 1 4 6 5\nf 2 3 6 5\nf 1 2 5\nf 4 3 6\n";

    #[test]
    fn a_flat_plate_has_no_picture_from_the_front_or_the_side() {
        let dir = temp("plate");
        let plate = write(&dir, "plate.obj", b"v 0 0 0\nv 20 0 0\nv 20 10 0\nv 0 10 0\nf 1 2 3 4\n");
        let path = plate.display().to_string();
        let mut app = DarkBurn::new(Prefs::default());
        for (view, flip) in [("front", false), ("front", true), ("side", false), ("side", true)] {
            let e = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "view": view, "flip": flip, "dpi": 25.4 }))
                .unwrap_err();
            assert!(e.contains("no height"), "{view} {flip}: {e}");
        }
        assert_eq!(app.editor.doc.len(), 0, "nothing goes in");
        assert!(!app.editor.history.can_undo());
        // From above it is an ordinary picture: all of it near, so all white.
        let v = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "dpi": 25.4 })).unwrap();
        assert_eq!(v["px"], json!([20, 10]), "{v}");
        let (shape, picture) = image_of(&app, v["created"].as_u64().unwrap());
        let ShapeKind::Image(img) = &shape.kind else { unreachable!() };
        assert!(img.height > 0.0);
        assert!(picture.as_luma16().unwrap().pixels().all(|p| p.0[0] == 65535));
    }

    #[test]
    fn flip_looks_from_the_other_side_through_the_command() {
        let dir = temp("flip");
        let wedge = write(&dir, "wedge.obj", WEDGE);
        let path = wedge.display().to_string();
        let mut app = DarkBurn::new(Prefs::default());
        let covered = |app: &DarkBurn, v: &Value, x: u32, y: u32| {
            let (_, picture) = image_of(app, v["created"].as_u64().unwrap());
            // Every covered pixel is equally near (a flat face), so covered = white.
            picture.as_luma16().unwrap().get_pixel(x, y).0[0] == 65535
        };
        let front = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "view": "front", "dpi": 25.4 })).unwrap();
        assert_eq!(front["px"], json!([20, 10]), "{front}");
        assert!(covered(&app, &front, 1, 1) && !covered(&app, &front, 18, 1), "front: the tall side on the left");
        let back =
            crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "view": "front", "flip": true, "dpi": 25.4 })).unwrap();
        assert_eq!(back["px"], json!([20, 10]), "{back}");
        assert!(!covered(&app, &back, 1, 1) && covered(&app, &back, 18, 1), "back: mirrored, the tall side on the right");
        // From above the slope runs from white on the left to black on the right; from below
        // the flat base is all one depth.
        let top = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "dpi": 25.4 })).unwrap();
        // Pixel centres run from x = 0.5 (9.75 high) to x = 19.5 (0.25 high).
        assert!((top["depth_mm"].as_f64().unwrap() - 9.5).abs() < 1e-3, "{top}");
        let bottom = crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "flip": true, "dpi": 25.4 })).unwrap();
        assert!(bottom["depth_mm"].as_f64().unwrap().abs() < 1e-6, "{bottom}");
        assert!(crate::api::execute_headless(&mut app, "image.depth_map", &json!({ "path": path, "flip": "yes" })).is_err());
    }

    #[test]
    fn an_agent_call_draws_on_a_worker_and_lands_when_it_is_done() {
        use darkburn_mcp::{Backend, ErrorCode, PendingState};
        let dir = temp("agent");
        let stl = write(&dir, "pyramid.stl", &mesh::write_binary_stl(&pyramid()));
        let broken = write(&dir, "broken.stl", &mesh::write_binary_stl(&pyramid())[..200]);
        let app = std::rc::Rc::new(std::cell::RefCell::new(DarkBurn::new(Prefs::default())));
        let mut b = crate::mcp_backend::McpBackend::new(app.clone(), Context::default(), true);
        let run = |b: &mut crate::mcp_backend::McpBackend, args: Value| {
            let e = b.execute("image.depth_map", args).unwrap_err();
            assert_eq!(e.code, ErrorCode::StillRunning, "{e:?}");
            assert_eq!(app.borrow().editor.doc.len(), 0, "nothing lands before the agent asks");
            assert!(b.busy(), "the window keeps drawing frames while it runs");
            let pid = e.pending_id.expect("a pending id");
            let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                match b.pending(&pid).unwrap() {
                    PendingState::Running { .. } => {}
                    PendingState::Done { result, .. } => return result,
                    PendingState::Unknown => panic!("the run vanished"),
                }
                assert!(std::time::Instant::now() < end, "timed out waiting for the agent's run");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        // A damaged file: the run fails and nothing goes in.
        let e = run(&mut b, json!({ "path": broken.display().to_string() })).unwrap_err();
        assert!(e.message.contains("ends early"), "{e:?}");
        let v = run(&mut b, json!({ "path": stl.display().to_string(), "width": 40, "dpi": 25.4 })).unwrap();
        assert_eq!(v["px"], json!([40, 40]), "{v}");
        assert_eq!(app.borrow().editor.doc.len(), 1);
        assert_eq!(app.borrow().editor.history.undo_label(), Some(UNDO_LABEL));
        assert!(!b.busy());
        // Bad arguments are refused at once, without a run.
        for bad in [
            json!({}),
            json!({ "path": stl.display().to_string(), "dpi": 0 }),
            json!({ "path": stl.display().to_string(), "depth_from": 2, "depth_to": 1 }),
        ] {
            let e = b.execute("image.depth_map", bad.clone()).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidParams, "{bad}: {e:?}");
        }
        assert!(!b.busy());
        assert_eq!(app.borrow().editor.doc.len(), 1);
    }

    #[test]
    fn the_window_speaks_norwegian() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        g.app().prefs.language = "nb".into();
        g.frames(3);
        assert_eq!(crate::i18n::language(), "nb");
        g.app().images.depth.dialog = Some(state_case_dialog());
        wait(&mut g, "the preview", &|g: &mut Gui| g.app().images.depth.dialog.as_ref().is_some_and(DepthDialog::preview_ready));
        g.frames(2);
        assert!(g.window_titles().iter().any(|t| t == "Importer 3D-modell"), "{:?}", g.window_titles());
        let texts = g.visible_texts();
        for want in ["Se fra", "Ovenfra", "Bredde", "Oppl\u{f8}sning", "Hvite", "Importer", "Avbryt"] {
            assert!(texts.iter().any(|t| t == want), "{want} is missing: {texts:?}");
        }
        assert!(texts.iter().any(|t| t.contains("6 trekanter")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("piksler")), "{texts:?}");
        for english in ["Look from", "Width", "Import 3D Model", "Cancel"] {
            assert!(!texts.iter().any(|t| t == english), "{english} is still English: {texts:?}");
        }
        g.screenshot("model-import-nb");
        g.close_dialogs();
        g.app().prefs.language = "en".into();
        g.frames(2);
        crate::i18n::set_language("en");
    }

    fn wait(g: &mut crate::ui_harness::Gui, what: &str, done: &dyn Fn(&mut crate::ui_harness::Gui) -> bool) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !done(g) {
            assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
            g.frames(2);
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    #[test]
    fn import_opens_the_window_and_ok_puts_in_the_height_picture() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        let dir = g.temp("model-import");
        std::fs::create_dir_all(&dir).unwrap();
        let ball = {
            // A dome on a square plate, so the preview shows round shading.
            let mut m = Mesh::default();
            let n = 48;
            let at = |i: usize, j: usize| {
                let (x, y) = (i as f32 / n as f32 * 40.0 - 20.0, j as f32 / n as f32 * 40.0 - 20.0);
                [x, y, (225.0 - x * x - y * y).max(0.0).sqrt() + 2.0]
            };
            for i in 0..n {
                for j in 0..n {
                    m.triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    m.triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
            m
        };
        let stl = write(&dir, "dome.stl", mesh::write_ascii_stl(&ball, "dome").as_bytes());
        ::rfd::script_answer(vec![stl]);
        g.run_command("file.import");
        g.frames(3);
        assert!(g.window_titles().iter().any(|t| t == "Import 3D Model"), "{:?}", g.window_titles());
        wait(&mut g, "the preview", &|g: &mut Gui| g.app().images.depth.dialog.as_ref().is_some_and(DepthDialog::preview_ready));
        g.frames(2);
        assert!(g.has_text("dome.stl"), "{:?}", g.visible_texts());
        assert!(g.visible_texts().iter().any(|t| t.contains("4608 triangles")), "{:?}", g.visible_texts());
        assert!(g.visible_texts().iter().any(|t| t.contains("400 x 400 pixels")), "{:?}", g.visible_texts());
        g.screenshot("model-import");
        // From the side the picture is 40 wide and 15 tall.
        {
            let mut app = g.app();
            let d = app.images.depth.dialog.as_mut().unwrap();
            d.settings.view = View::Side;
        }
        wait(&mut g, "the side preview", &|g: &mut Gui| g.app().images.depth.dialog.as_ref().is_some_and(DepthDialog::preview_ready));
        g.frames(2);
        assert!(g.visible_texts().iter().any(|t| t.contains("40.00 x 15.00 mm")), "{:?}", g.visible_texts());
        g.click_label("Import");
        wait(&mut g, "the picture", &|g: &mut Gui| g.app().images.depth.dialog.is_none());
        let app = g.app();
        assert_eq!(app.editor.doc.len(), 1);
        assert_eq!(app.editor.history.undo_label(), Some(UNDO_LABEL));
        let s = app.editor.doc.iter().next().unwrap();
        assert!(s.is_image());
        assert_eq!(app.editor.doc.layer(s.layer).main().mode, Mode::Image);
    }

    #[test]
    fn cancel_leaves_the_design_as_it_was_and_the_next_model_follows() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        let dir = g.temp("model-cancel");
        std::fs::create_dir_all(&dir).unwrap();
        let a = write(&dir, "first.stl", &mesh::write_binary_stl(&pyramid()));
        let b = write(&dir, "second.stl", b"solid cut off\nfacet normal 0 0 1\n");
        let ctx = g.ctx();
        crate::files::drop_files(&mut g.app(), &ctx, vec![a, b], None);
        g.frames(3);
        assert!(g.has_text("first.stl"), "{:?}", g.visible_texts());
        g.click_label("Cancel");
        g.frames(3);
        // The second file follows, and says it is damaged; Import stays off.
        assert!(g.has_text("second.stl"), "{:?}", g.visible_texts());
        wait(&mut g, "the read", &|g: &mut Gui| g.app().images.depth.dialog.as_ref().is_some_and(|d| d.error.is_some()));
        g.frames(2);
        assert!(g.visible_texts().iter().any(|t| t.contains("ends early")), "{:?}", g.visible_texts());
        g.click_label("Import");
        g.frames(2);
        assert!(g.app().images.depth.dialog.is_some());
        g.close_dialogs();
        let app = g.app();
        assert_eq!(app.editor.doc.len(), 0);
        assert!(!app.editor.history.can_undo());
    }
}
