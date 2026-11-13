//! Trace Image Files: several picture files traced with the same settings in one go [IMG-21].
//!
//! The files are read and traced on a worker thread that can be stopped between files. Each
//! file becomes one group of path shapes (an outline with its holes is one shape), and the
//! groups are laid out in a row, or a grid when there are many, without overlapping, on the
//! layer Trace Image would use. Everything lands as one undo step. A file that cannot be read
//! is left out and named in the report. Files are held to the importer's limits (file size,
//! picture size, decoder memory), and pictures larger than the trace limit are shrunk first.
//!
//! `image.trace_files` is the headless command (a list of paths and the trace settings);
//! `tools.trace_files` asks for the files and opens the window, which steps a preview through
//! them before tracing.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

use darkburn_core::kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Size};
use darkburn_core::{Editor, Shape, ShapeId, ShapeKind};
use darkburn_raster::GrayF;
use darkburn_ui::ui_kit;
use egui::{Align2, Context, Id, TextureHandle, TextureOptions, Ui};
use serde_json::{Value, json};

use crate::image_ops::{self, TraceArgs};
use crate::image_ui;
use crate::numeric::{self, Num};
use crate::shell::DarkBurn;

/// Most files one run takes.
pub const MAX_FILES: usize = 500;
/// Space between the traced files, mm, unless asked otherwise.
pub const DEFAULT_GAP_MM: f64 = 5.0;
/// Above this many outlines in one file, it becomes a single shape instead of a group.
const MAX_ISLANDS: usize = 1000;

// ----- Reading and tracing one file -----

/// A picture read for tracing: its grey pixels and its size in mm, as an import would place it.
pub struct Loaded {
    pub gray: GrayF,
    pub size_mm: (f64, f64),
}

/// The file name of `path`, for messages.
pub fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Reads picture `path` the way Import does (same file size, picture size and decoder limits,
/// same size in mm on a work area of `page`), shrunk to at most `max_px` pixels.
pub fn load(path: &Path, page: Size, max_px: u64) -> Result<Loaded, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not be read ({e})"))?;
    if !meta.is_file() {
        return Err("is not a file".into());
    }
    if meta.len() > crate::formats::MAX_FILE_BYTES {
        return Err(format!("is larger than {} MB", crate::formats::MAX_FILE_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("could not be read ({e})"))?;
    let decoded = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<Loaded, String> {
        // The importer checks the format and the picture size from the header before any
        // pixels are decoded, and gives the size in mm.
        let opts = crate::import::Options { page, ..Default::default() };
        let imported = crate::import::bitmap(&bytes, &opts, None)?;
        let size_mm = match imported.shapes.first().map(|s| &s.kind) {
            Some(ShapeKind::Image(img)) => (img.width, img.height),
            _ => return Err("this is not an image DarkBurn can read".into()),
        };
        let mut img = crate::images::decode(&bytes)?;
        let px = u64::from(img.width()) * u64::from(img.height());
        if px > max_px.max(1) {
            let k = (max_px.max(1) as f64 / px as f64).sqrt();
            let (w, h) = (((f64::from(img.width()) * k) as u32).max(1), ((f64::from(img.height()) * k) as u32).max(1));
            img = img.resize_exact(w, h, image::imageops::FilterType::Triangle);
        }
        Ok(Loaded { gray: GrayF::from_dynamic(&img), size_mm })
    }));
    decoded.unwrap_or_else(|_| Err("the image could not be read".into()))
}

/// The paths tracing `l` gives, in its own millimetres (centred on the origin, Y up).
pub fn trace_loaded(l: &Loaded, args: &TraceArgs, max_px: u64) -> Vec<BezPath> {
    let (pw, ph) = (f64::from(l.gray.width.max(1)), f64::from(l.gray.height.max(1)));
    let (w, h) = l.size_mm;
    let to_mm = Affine::new([w / pw, 0.0, 0.0, -h / ph, -w / 2.0, h / 2.0]);
    image_ops::trace_gray_limited(&l.gray, to_mm, args, max_px)
}

/// What tracing one file gave.
pub struct FileResult {
    pub path: PathBuf,
    /// The paths (centred on the origin) and the picture's size in mm, or why it was skipped.
    pub traced: Result<(Vec<BezPath>, (f64, f64)), String>,
}

fn trace_file(path: &Path, args: &TraceArgs, page: Size) -> Result<(Vec<BezPath>, (f64, f64)), String> {
    let l = load(path, page, image_ops::TRACE_MAX_PIXELS)?;
    let paths = std::panic::catch_unwind(AssertUnwindSafe(|| trace_loaded(&l, args, image_ops::TRACE_MAX_PIXELS)))
        .map_err(|_| "the image could not be traced".to_string())?;
    if paths.is_empty() {
        return Err("nothing was found to trace".into());
    }
    Ok((paths, l.size_mm))
}

/// Traces `paths` one after another; `done` counts the files finished. None when `cancel` was
/// set before the end.
pub fn trace_files(paths: &[PathBuf], args: &TraceArgs, page: Size, cancel: &AtomicBool, done: &AtomicUsize) -> Option<Vec<FileResult>> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        out.push(FileResult { path: path.clone(), traced: trace_file(path, args, page) });
        done.fetch_add(1, Ordering::Relaxed);
    }
    (!cancel.load(Ordering::Relaxed)).then_some(out)
}

// ----- Worker -----

/// A run on a worker thread, so the window stays live, shows progress and can stop it.
pub struct TraceJob {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
    handle: Option<JoinHandle<Option<Vec<FileResult>>>>,
}

impl TraceJob {
    pub fn start(paths: Vec<PathBuf>, args: TraceArgs, page: Size) -> TraceJob {
        Self::start_with(paths, args, page, false)
    }

    /// [`TraceJob::start`] with the cancel flag already set (for tests).
    pub fn start_with(paths: Vec<PathBuf>, args: TraceArgs, page: Size, cancelled: bool) -> TraceJob {
        let cancel = Arc::new(AtomicBool::new(cancelled));
        let done = Arc::new(AtomicUsize::new(0));
        let total = paths.len();
        let (c, d) = (cancel.clone(), done.clone());
        let handle = std::thread::spawn(move || trace_files(&paths, &args, page, &c, &d));
        TraceJob { cancel, done, total, handle: Some(handle) }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// (files done, files in all).
    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }

    pub fn finished(&self) -> bool {
        self.handle.as_ref().is_none_or(|h| h.is_finished())
    }

    /// Wait for the end: the results, or None when it was stopped.
    pub fn take(mut self) -> Option<Vec<FileResult>> {
        self.handle.take().and_then(|h| h.join().ok()).flatten()
    }
}

// ----- Into the document -----

/// Splits traced outlines into shapes: each outer outline with the holes directly inside it.
/// Open (sketch) lines stand alone. Very busy pictures stay one shape.
pub fn islands(paths: Vec<BezPath>) -> Vec<BezPath> {
    let paths: Vec<BezPath> = paths.into_iter().filter(|p| !p.elements().is_empty()).collect();
    if paths.len() > MAX_ISLANDS {
        let mut all = BezPath::new();
        paths.into_iter().for_each(|p| all.extend(p));
        return vec![all];
    }
    let (closed, open): (Vec<BezPath>, Vec<BezPath>) = paths.into_iter().partition(|p| matches!(p.elements().last(), Some(PathEl::ClosePath)));
    let n = closed.len();
    let areas: Vec<f64> = closed.iter().map(|p| p.area()).collect();
    let boxes: Vec<Rect> = closed.iter().map(|p| p.bounding_box()).collect();
    // The largest outline is an outer one; holes wind the other way.
    let outer_sign = (0..n).max_by(|a, b| areas[*a].abs().total_cmp(&areas[*b].abs())).is_none_or(|i| areas[i] >= 0.0);
    let is_outer = |i: usize| (areas[i] >= 0.0) == outer_sign;
    let inside = |a: Rect, b: Rect| a.x0 <= b.x0 + 1e-9 && a.y0 <= b.y0 + 1e-9 && a.x1 >= b.x1 - 1e-9 && a.y1 >= b.y1 - 1e-9;
    let mut owner: Vec<usize> = (0..n).collect();
    for h in (0..n).filter(|i| !is_outer(*i)) {
        let Some(PathEl::MoveTo(start)) = closed[h].elements().first().copied() else { continue };
        let best = (0..n)
            .filter(|o| is_outer(*o) && inside(boxes[*o], boxes[h]) && closed[*o].winding(start) != 0)
            .min_by(|a, b| areas[*a].abs().total_cmp(&areas[*b].abs()));
        if let Some(o) = best {
            owner[h] = o;
        }
    }
    let mut merged: Vec<Option<BezPath>> = vec![None; n];
    for (i, p) in closed.into_iter().enumerate() {
        merged[owner[i]].get_or_insert_with(BezPath::new).extend(p);
    }
    merged.into_iter().flatten().chain(open).collect()
}

/// Centres for blocks of `sizes` (mm) laid out `columns` wide (0: a row of up to four, else a
/// square-ish grid) with `gap` between them, the whole centred on `centre`. Each column is as
/// wide as its widest block and each row as tall as its tallest, so nothing overlaps.
pub fn layout(sizes: &[(f64, f64)], columns: usize, gap: f64, centre: Point) -> Vec<Point> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    let cols = if columns > 0 {
        columns.min(n)
    } else if n <= 4 {
        n
    } else {
        (n as f64).sqrt().ceil() as usize
    };
    let rows = n.div_ceil(cols);
    let gap = if gap.is_finite() { gap.max(0.0) } else { DEFAULT_GAP_MM };
    let mut col_w = vec![0.0f64; cols];
    let mut row_h = vec![0.0f64; rows];
    for (i, (w, h)) in sizes.iter().enumerate() {
        col_w[i % cols] = col_w[i % cols].max(*w);
        row_h[i / cols] = row_h[i / cols].max(*h);
    }
    let total_w = col_w.iter().sum::<f64>() + gap * (cols - 1) as f64;
    let total_h = row_h.iter().sum::<f64>() + gap * (rows - 1) as f64;
    let (left, top) = (centre.x - total_w / 2.0, centre.y + total_h / 2.0);
    (0..n)
        .map(|i| {
            let (c, r) = (i % cols, i / cols);
            let x = left + col_w[..c].iter().sum::<f64>() + gap * c as f64 + col_w[c] / 2.0;
            let y = top - row_h[..r].iter().sum::<f64>() - gap * r as f64 - row_h[r] / 2.0;
            Point::new(x, y)
        })
        .collect()
}

/// What a run put into the document.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// One shape per traced file (a group, or a path when the file gave one outline).
    pub created: Vec<ShapeId>,
    /// (file name, why) for every file left out.
    pub skipped: Vec<(String, String)>,
    pub nodes: usize,
}

/// The skipped files as lines of a message.
pub fn skipped_text(skipped: &[(String, String)]) -> String {
    skipped.iter().map(|(f, e)| format!("{f}: {e}")).collect::<Vec<_>>().join("\n")
}

/// Puts the traced files into the document as one undo step "Trace files", laid out by
/// [`layout`] around the middle of `page`, and selects them.
pub fn apply(e: &mut Editor, results: Vec<FileResult>, args: &TraceArgs, gap: f64, columns: usize, page: Size) -> Result<Outcome, String> {
    let mut skipped = Vec::new();
    let mut good = Vec::new();
    for r in results {
        match r.traced {
            Ok(t) => good.push(t),
            Err(why) => skipped.push((name_of(&r.path), why)),
        }
    }
    if good.is_empty() {
        let why = if skipped.is_empty() { "no files were given".to_string() } else { skipped_text(&skipped).replace('\n', "; ") };
        return Err(format!("nothing was traced: {why}"));
    }
    let sizes: Vec<(f64, f64)> = good.iter().map(|(_, s)| *s).collect();
    let centres = layout(&sizes, columns, gap, Point::new(page.width / 2.0, page.height / 2.0));
    let layer = image_ops::trace_layer(&e.doc, args.layer, e.current_layer);
    let nodes = good.iter().map(|(p, _)| image_ops::node_count(p)).sum();
    let mut created = Vec::new();
    let done = e.edit("Trace files", |d, sel| {
        for ((paths, _), at) in good.into_iter().zip(centres) {
            let mut parts: Vec<Shape> = islands(paths)
                .into_iter()
                .map(|mut p| {
                    p.apply_affine(Affine::translate(at.to_vec2()));
                    Shape::path(p, layer)
                })
                .collect();
            let shape = if parts.len() == 1 { parts.remove(0) } else { Shape::new(ShapeKind::Group { children: parts }, Affine::IDENTITY, layer) };
            created.push(d.add(shape));
        }
        sel.set(created.iter().copied());
        true
    });
    if !done {
        return Err("nothing was done".into());
    }
    Ok(Outcome { created, skipped, nodes })
}

fn page_of(app: &DarkBurn) -> Size {
    Size::new(f64::from(app.prefs.page_width), f64::from(app.prefs.page_height))
}

/// The settings a run takes from command arguments: the trace settings, `gap` and `columns`.
fn layout_args(a: &Value) -> (f64, usize) {
    let gap = a.get("gap").and_then(Value::as_f64).filter(|g| g.is_finite()).map_or(DEFAULT_GAP_MM, |g| g.clamp(0.0, 1000.0));
    let columns = a.get("columns").and_then(Value::as_u64).map_or(0, |c| c.min(100) as usize);
    (gap, columns)
}

/// `image.trace_files`: traces the files in `paths` with the trace settings in `a`, now.
pub fn execute(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let list = a.get("paths").and_then(Value::as_array).ok_or("argument paths must be a list of file paths")?;
    let paths: Vec<PathBuf> = list.iter().map(|p| p.as_str().map(PathBuf::from).ok_or("argument paths must hold text")).collect::<Result<_, _>>()?;
    if paths.is_empty() {
        return Err("argument paths is empty".into());
    }
    if paths.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} files can be traced at once"));
    }
    let args: TraceArgs = serde_json::from_value(a.clone()).map_err(|e| format!("arguments: {e}"))?;
    let (gap, columns) = layout_args(a);
    let page = page_of(app);
    let results = trace_files(&paths, &args, page, &AtomicBool::new(false), &AtomicUsize::new(0)).unwrap_or_default();
    let out = apply(&mut app.editor, results, &args, gap, columns, page)?;
    Ok(json!({
        "created": out.created.iter().map(|s| s.0).collect::<Vec<_>>(),
        "skipped": out.skipped.iter().map(|(f, e)| json!({ "file": f, "error": e })).collect::<Vec<_>>(),
        "nodes": out.nodes,
    }))
}

// ----- Window -----

/// The preview of the file shown in the window.
#[derive(Default)]
struct Preview {
    file: Option<PathBuf>,
    loading: Option<JoinHandle<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    texture: Option<TextureHandle>,
    paths: Vec<BezPath>,
    paths_for: Option<String>,
    error: Option<String>,
}

/// The open Trace Image Files window.
pub struct MultiTraceDialog {
    pub files: Vec<PathBuf>,
    /// The file the preview shows.
    pub current: usize,
    pub args: TraceArgs,
    pub gap: f64,
    pub job: Option<TraceJob>,
    pub notice: Option<String>,
    preview: Preview,
}

impl MultiTraceDialog {
    pub fn new(files: Vec<PathBuf>, args: TraceArgs) -> MultiTraceDialog {
        MultiTraceDialog { files, current: 0, args, gap: DEFAULT_GAP_MM, job: None, notice: None, preview: Preview::default() }
    }

    /// Adds `more`, leaving out files already in the list.
    pub fn add(&mut self, more: Vec<PathBuf>) {
        for p in more {
            if !self.files.contains(&p) && self.files.len() < MAX_FILES {
                self.files.push(p);
            }
        }
    }

    /// Steps the preview by `by` files (wrapping round).
    pub fn step(&mut self, by: isize) {
        let n = self.files.len() as isize;
        if n > 0 {
            self.current = (self.current as isize + by).rem_euclid(n) as usize;
        }
    }

    /// The file the preview shows.
    pub fn shown(&self) -> Option<&Path> {
        self.files.get(self.current).map(PathBuf::as_path)
    }

    /// Whether the preview of the shown file has been read (or found unreadable).
    #[cfg(test)]
    pub fn preview_ready(&self) -> bool {
        self.preview.loading.is_none() && (self.preview.loaded.is_some() || self.preview.error.is_some())
    }

    /// Outlines in the preview.
    #[cfg(test)]
    pub fn preview_outlines(&self) -> usize {
        self.preview.paths.len()
    }

    fn key(&self) -> String {
        let a = &self.args;
        format!(
            "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            a.threshold, a.lower_cutoff, a.ignore_smaller, a.smoothness, a.optimize, a.sketch, a.sketch_radius, a.sketch_sensitivity
        )
    }

    /// Reads the shown file for the preview on a worker, and traces it again when the settings change.
    fn refresh(&mut self, ctx: &Context, page: Size) {
        let want = self.shown().map(Path::to_path_buf);
        if self.preview.file != want {
            self.preview = Preview { file: want.clone(), ..Preview::default() };
            if let Some(path) = want {
                self.preview.loading = Some(std::thread::spawn(move || load(&path, page, image_ui::TRACE_PREVIEW_PIXELS)));
            }
        }
        if self.preview.loading.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(h) = self.preview.loading.take()
        {
            match h.join().unwrap_or_else(|_| Err("the image could not be read".into())) {
                Ok(l) => {
                    let luma = l.gray.to_luma8();
                    let image = egui::ColorImage::from_gray([luma.width() as usize, luma.height() as usize], luma.as_raw());
                    self.preview.texture = Some(ctx.load_texture("db_trace_files_base", image, TextureOptions::LINEAR));
                    self.preview.loaded = Some(l);
                }
                Err(e) => self.preview.error = Some(e),
            }
        }
        if self.preview.loading.is_some() {
            ctx.request_repaint();
        }
        let key = self.key();
        if let Some(l) = &self.preview.loaded
            && self.preview.paths_for.as_ref() != Some(&key)
        {
            self.preview.paths = trace_loaded(l, &self.args, image_ui::TRACE_PREVIEW_PIXELS);
            self.preview.paths_for = Some(key);
        }
    }
}

fn pick_files() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new().add_filter("Images", crate::import::BITMAP_EXTENSIONS).pick_files()
}

/// `tools.trace_files`: asks for picture files and opens the window with `args`.
pub fn open(app: &mut DarkBurn, args: TraceArgs) -> Result<(), String> {
    let mut files = pick_files().ok_or("no files were chosen")?;
    files.truncate(MAX_FILES);
    app.images.multi = Some(MultiTraceDialog::new(files, args));
    Ok(())
}

/// What the window's body asked for.
#[derive(Default)]
struct Asked {
    add: bool,
    stop: bool,
}

fn body(ui: &mut Ui, d: &mut MultiTraceDialog) -> Asked {
    let mut asked = Asked::default();
    let running = d.job.as_ref().map(TraceJob::progress);
    let mut rule = 0.0;
    let row = ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(320.0);
            ui.add_enabled_ui(running.is_none(), |ui| {
                ui_kit::section_heading(ui, "Files");
                egui::ScrollArea::vertical().id_salt("trace_files_list").max_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
                    let mut pick = None;
                    for (i, f) in d.files.iter().enumerate() {
                        if ui_kit::list_picker(ui, &format!("trace_files:{i}"), &name_of(f), i == d.current).clicked() {
                            pick = Some(i);
                        }
                    }
                    if let Some(i) = pick {
                        d.current = i;
                    }
                });
                ui.horizontal(|ui| {
                    if ui_kit::button(ui, "Previous", ui_kit::Button::Small).clicked() {
                        d.step(-1);
                    }
                    if ui_kit::button(ui, "Next", ui_kit::Button::Small).clicked() {
                        d.step(1);
                    }
                    if ui_kit::button(ui, "Add files...", ui_kit::Button::Small).clicked() {
                        asked.add = true;
                    }
                    if ui_kit::button(ui, "Remove", ui_kit::Button::Enabled(d.files.len() > 1)).clicked() && d.files.len() > 1 {
                        d.files.remove(d.current);
                        d.current = d.current.min(d.files.len() - 1);
                    }
                });
                if !d.files.is_empty() {
                    ui_kit::caption(ui, "Files", &format!("Showing file {} of {}", d.current + 1, d.files.len()));
                }
                ui_kit::section_heading(ui, "Trace");
                image_ui::trace_settings(ui, &mut d.args);
                numeric::row(ui, "Space between", &mut d.gap, &Num::new(0.0..=1000.0, 1, " mm").speed(0.1));
                ui_kit::section_heading(ui, "Preview");
                let mut fade = f64::from(d.args.fade);
                if numeric::row(ui, "Fade image", &mut fade, &Num::new(0.0..=1.0, 2, "").speed(0.01)).changed {
                    d.args.fade = fade as f32;
                }
                ui_kit::checkbox(ui, "Show points", &mut d.args.show_points);
            });
            if let Some(e) = &d.preview.error {
                ui_kit::status_line(ui, &format!("{}: {e}", d.shown().map(name_of).unwrap_or_default()), ui_kit::StatusTone::Warning);
            } else if d.preview.loaded.is_some() {
                let paths = &d.preview.paths;
                ui_kit::caption(ui, "Cutoff", &format!("{} outlines, {} nodes", paths.len(), image_ops::node_count(paths)));
            }
            if let Some((done, total)) = running {
                let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                if ui_kit::progress_action(ui, "trace_files", "Tracing", &format!("{done} of {total}"), frac, "Stop").clicked() {
                    asked.stop = true;
                }
            }
            if let Some(n) = &d.notice {
                ui_kit::status_line(ui, n, ui_kit::StatusTone::Error);
            }
        });
        rule = image_ui::column_gap(ui);
        ui.vertical(|ui| {
            let size = d.preview.loaded.as_ref().map_or((100.0, 100.0), |l| l.size_mm);
            image_ui::trace_preview(ui, d.preview.texture.as_ref(), size, &d.preview.paths, (d.args.fade, d.args.show_points), None);
        });
    });
    image_ui::column_rule(ui, rule, row.response.rect);
    asked
}

/// Draws the Trace Image Files window; called once a frame from the image windows.
pub fn dialog(ctx: &Context, app: &mut DarkBurn) {
    let Some(mut d) = app.images.multi.take() else { return };
    let page = page_of(app);
    d.refresh(ctx, page);
    let mut asked = Asked::default();
    let valid = d.job.is_none() && !d.files.is_empty();
    let (closed, action) = ui_kit::Window::new(Id::unique("db_trace_files"), "trace", "Trace Image Files")
        .cards(2.9)
        .fields()
        .placed(Align2::CENTER_CENTER, crate::shell::room_under_the_bar(ctx).center())
        .actions("Trace Files", "Cancel", None)
        .show(ctx, valid, |ui| image_ui::under_the_bar(ui, |ui| asked = body(ui, &mut d)));
    if closed || action == ui_kit::Action::Cancel {
        if let Some(job) = d.job.take() {
            job.cancel();
        }
        return;
    }
    if asked.add
        && let Some(more) = pick_files()
    {
        d.add(more);
    }
    if asked.stop
        && let Some(job) = &d.job
    {
        job.cancel();
    }
    if action == ui_kit::Action::Ok && d.job.is_none() && !d.files.is_empty() {
        d.notice = None;
        d.job = Some(TraceJob::start(d.files.clone(), d.args.clone(), page));
    }
    if d.job.as_ref().is_some_and(TraceJob::finished)
        && let Some(job) = d.job.take()
    {
        match job.take() {
            Some(results) => match apply(&mut app.editor, results, &d.args, d.gap, 0, page) {
                Ok(out) => {
                    app.note = Some(format!("Traced {} files", out.created.len()));
                    if !out.skipped.is_empty() {
                        crate::files::show_message(app, "Some files were not traced", skipped_text(&out.skipped));
                    }
                    return;
                }
                Err(e) => d.notice = Some(e),
            },
            None => d.notice = Some("Stopped, nothing changed".into()),
        }
    }
    if d.job.is_some() {
        ctx.request_repaint();
    }
    app.images.multi = Some(d);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::testutil::png;
    use crate::prefs::Prefs;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkburn-multitrace-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Two dark squares, the first with a light hole: two outlines plus a hole.
    fn two_squares(w: u32, h: u32) -> Vec<u8> {
        png(w, h, |x, y| {
            let in_a = (4..24).contains(&x) && (4..24).contains(&y);
            let hole = (10..18).contains(&x) && (10..18).contains(&y);
            let in_b = (30..44).contains(&x) && (8..20).contains(&y);
            if (in_a && !hole) || in_b { 0 } else { 255 }
        })
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    /// A PNG header claiming `w` x `h` pixels with no pixels behind it.
    fn png_claiming(w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc32(&body).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 0, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IEND", &[]);
        out
    }

    fn bbox(app: &DarkBurn, id: u64) -> Rect {
        app.editor.doc.get(ShapeId(id)).unwrap().bbox()
    }

    #[test]
    fn files_become_groups_side_by_side_in_one_undo_step_and_bad_files_are_named() {
        let dir = temp("headless");
        let a = write(&dir, "a.png", &two_squares(48, 28));
        let b = write(&dir, "b.png", &two_squares(48, 28));
        let broken = write(&dir, "broken.png", b"this is not a picture");
        let missing = dir.join("missing.png");
        let mut app = DarkBurn::new(Prefs::default());
        let paths: Vec<String> = [&a, &broken, &b, &missing].iter().map(|p| p.display().to_string()).collect();
        let v = crate::api::execute_headless(&mut app, "image.trace_files", &json!({ "paths": paths, "threshold": 128 })).unwrap();
        let created: Vec<u64> = v["created"].as_array().unwrap().iter().map(|i| i.as_u64().unwrap()).collect();
        assert_eq!(created.len(), 2, "{v}");
        let skipped: Vec<String> = v["skipped"].as_array().unwrap().iter().map(|s| s["file"].as_str().unwrap().to_string()).collect();
        assert_eq!(skipped, vec!["broken.png".to_string(), "missing.png".to_string()]);
        assert!(v["skipped"][0]["error"].as_str().is_some_and(|e| !e.is_empty()));
        assert!(v["nodes"].as_u64().unwrap() > 0);
        for id in &created {
            let s = app.editor.doc.get(ShapeId(*id)).unwrap();
            // The square with its hole is one shape, the other square another.
            let ShapeKind::Group { children } = &s.kind else { panic!("one group per file") };
            assert_eq!(children.len(), 2);
            assert!(children.iter().all(|c| c.layer == s.layer));
        }
        let (ba, bb) = (bbox(&app, created[0]), bbox(&app, created[1]));
        assert!(ba.intersect(bb).area() <= 0.0, "{ba:?} overlaps {bb:?}");
        assert!((ba.center().y - bb.center().y).abs() < 1e-6, "a row");
        assert!(ba.center().x < bb.center().x, "in the order given");
        let mut sel = app.editor.selected();
        sel.sort();
        assert_eq!(sel, created.iter().map(|i| ShapeId(*i)).collect::<Vec<_>>());
        assert_eq!(app.editor.history.undo_label(), Some("Trace files"));
        assert!(app.editor.undo());
        assert_eq!(app.editor.doc.len(), 0, "one undo takes all of it away");
    }

    #[test]
    fn nothing_readable_is_an_error_that_names_the_files() {
        let dir = temp("none");
        let bad = write(&dir, "junk.jpg", b"\xFF\xD8 not really");
        let mut app = DarkBurn::new(Prefs::default());
        let e = crate::api::execute_headless(&mut app, "image.trace_files", &json!({ "paths": [bad.display().to_string()] })).unwrap_err();
        assert!(e.contains("junk.jpg"), "{e}");
        assert_eq!(app.editor.doc.len(), 0);
        assert!(!app.editor.history.can_undo());
        assert!(crate::api::execute_headless(&mut app, "image.trace_files", &json!({ "paths": [] })).is_err());
        assert!(crate::api::execute_headless(&mut app, "image.trace_files", &json!({})).is_err());
        assert!(crate::api::execute_headless(&mut app, "image.trace_files", &json!({ "paths": [3] })).is_err());
    }

    #[test]
    fn oversized_and_huge_files_are_refused_by_the_import_limits() {
        let dir = temp("huge");
        let wide = write(&dir, "wide.png", &png_claiming(crate::images::MAX_IMPORT_SIDE + 1, 1));
        let many = write(&dir, "many.png", &png_claiming(20_000, 20_000));
        let page = Size::new(400.0, 400.0);
        // Refused from the header, before any pixels are decoded.
        assert!(load(&wide, page, image_ops::TRACE_MAX_PIXELS).is_err());
        assert!(load(&many, page, image_ops::TRACE_MAX_PIXELS).is_err());
        let mut app = DarkBurn::new(Prefs::default());
        let v = crate::api::execute_headless(&mut app, "image.trace_files", &json!({ "paths": [many.display().to_string()] }));
        assert!(v.is_err_and(|e| e.contains("many.png")));
        // A large picture is shrunk to the limit before it is traced.
        let big = write(&dir, "big.png", &png(400, 300, |x, _| if x < 200 { 0 } else { 255 }));
        let l = load(&big, page, 30_000).unwrap();
        assert!(u64::from(l.gray.width) * u64::from(l.gray.height) <= 30_000);
        assert!((l.size_mm.0 - 40.0).abs() < 1e-6 && (l.size_mm.1 - 30.0).abs() < 1e-6, "{:?}", l.size_mm);
        assert!(!trace_loaded(&l, &TraceArgs::default(), 30_000).is_empty());
    }

    #[test]
    fn a_stopped_run_changes_nothing() {
        let dir = temp("stop");
        let a = write(&dir, "a.png", &two_squares(48, 28));
        let job = TraceJob::start_with(vec![a.clone()], TraceArgs::default(), Size::new(400.0, 400.0), true);
        assert!(job.take().is_none());
        let job = TraceJob::start(vec![a], TraceArgs::default(), Size::new(400.0, 400.0));
        assert_eq!(job.progress().1, 1);
        let results = job.take().expect("a run that is not stopped finishes");
        assert_eq!(results.len(), 1);
        assert!(results[0].traced.is_ok());
    }

    #[test]
    fn layout_never_overlaps_and_wraps_into_a_grid() {
        let sizes = [(10.0, 5.0), (30.0, 20.0), (5.0, 40.0), (12.0, 12.0), (8.0, 3.0), (25.0, 25.0), (1.0, 1.0)];
        for n in 1..=sizes.len() {
            for columns in [0, 1, 2, 3] {
                let centres = layout(&sizes[..n], columns, 2.0, Point::new(100.0, 100.0));
                let rects: Vec<Rect> = centres.iter().zip(&sizes[..n]).map(|(c, (w, h))| Rect::from_center_size(*c, (*w, *h))).collect();
                for i in 0..n {
                    for j in i + 1..n {
                        assert!(rects[i].intersect(rects[j]).area() <= 1e-9, "n={n} columns={columns}: {i} and {j} overlap");
                    }
                }
                let all = rects.iter().copied().reduce(|a, b| a.union(b)).unwrap();
                assert!((all.center() - Point::new(100.0, 100.0)).hypot() < 1e-6, "centred");
            }
        }
        // Up to four in a row, then a grid three wide for seven.
        let row = layout(&sizes[..4], 0, 2.0, Point::ZERO);
        assert!(row.windows(2).all(|w| w[0].x < w[1].x));
        let grid = layout(&sizes, 0, 2.0, Point::ZERO);
        assert!((grid[3].x - grid[0].x).abs() < 1e-9 && grid[3].y < grid[0].y);
    }

    #[test]
    fn holes_stay_with_their_outline_and_open_lines_stand_alone() {
        // Counter-clockwise (Y up) when `ccw`, else clockwise.
        let rect = |r: Rect, ccw: bool| {
            let mut corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)];
            if !ccw {
                corners.reverse();
            }
            let mut p = BezPath::new();
            p.move_to(corners[0]);
            corners[1..].iter().for_each(|c| p.line_to(*c));
            p.close_path();
            p
        };
        let outer = rect(Rect::new(0.0, 0.0, 10.0, 10.0), true);
        let hole = rect(Rect::new(2.0, 2.0, 8.0, 8.0), false);
        let island = rect(Rect::new(4.0, 4.0, 6.0, 6.0), true);
        let other = rect(Rect::new(20.0, 0.0, 30.0, 10.0), true);
        let mut line = BezPath::new();
        line.move_to((0.0, 20.0));
        line.line_to((5.0, 25.0));
        let parts = islands(vec![hole, other, island, line, outer]);
        assert_eq!(parts.len(), 4, "outer+hole, other, island, line");
        let with_hole = parts.iter().find(|p| p.elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count() == 2).unwrap();
        assert!((with_hole.bounding_box().width() - 10.0).abs() < 1e-9);
        let busy: Vec<BezPath> = (0..MAX_ISLANDS + 1).map(|i| rect(Rect::new(i as f64 * 2.0, 0.0, i as f64 * 2.0 + 1.0, 1.0), true)).collect();
        assert_eq!(islands(busy).len(), 1);
    }

    #[test]
    fn the_window_steps_the_preview_and_traces_every_file() {
        use crate::ui_harness::Gui;
        let mut g = Gui::standard();
        let dir = g.temp("multi");
        std::fs::create_dir_all(&dir).unwrap();
        let a = write(&dir, "first.png", &two_squares(48, 28));
        let b = write(&dir, "second.png", &png(40, 40, |x, y| if (x as i32 - 20).pow(2) + (y as i32 - 20).pow(2) < 196 { 0 } else { 255 }));
        let bad = write(&dir, "unreadable.png", b"nope");
        ::rfd::script_answer(vec![a, b, bad]);
        g.run_command("tools.trace_files");
        assert!(g.window_titles().iter().any(|t| t == "Trace Image Files"), "{:?}", g.window_titles());
        assert!(g.dialog_log().iter().any(|l| l.contains("pick_files")), "{:?}", g.dialog_log());
        let wait = |g: &mut Gui, what: &str, done: &dyn Fn(&mut Gui) -> bool| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !done(g) {
                assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
                g.frames(2);
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        };
        let ready = |g: &mut Gui| g.app().images.multi.as_ref().is_some_and(|d| d.preview_ready() && d.preview_outlines() > 0);
        wait(&mut g, "the first preview", &ready);
        assert!(g.has_text("Showing file 1 of 3"));
        g.click_label("Next");
        g.frames(2);
        assert_eq!(g.app().images.multi.as_ref().map(|d| d.current), Some(1));
        wait(&mut g, "the second preview", &ready);
        assert!(g.has_text("Showing file 2 of 3"));
        g.screenshot("multi-trace");
        g.click_label("Trace Files");
        wait(&mut g, "the run", &|g: &mut Gui| g.app().images.multi.is_none());
        {
            let app = g.app();
            assert_eq!(app.editor.doc.len(), 2, "one shape per readable file");
            assert_eq!(app.editor.history.undo_label(), Some("Trace files"));
        }
        g.frames(3);
        assert!(g.window_titles().iter().any(|t| t.contains("not traced")), "{:?}", g.window_titles());
        assert!(g.visible_texts().iter().any(|t| t.contains("unreadable.png")), "the skipped file is named: {:?}", g.visible_texts());
    }
}
