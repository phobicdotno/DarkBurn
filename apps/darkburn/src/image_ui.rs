//! The image commands and their windows: Adjust Image (with presets), Trace Image, Convert to
//! Bitmap and the scanning offset table [TOOLS-19, TOOLS-20, TOOLS-22, IMG-16..IMG-20, IMG-23,
//! EDIT-12, EDIT-22..EDIT-28, FILE-13].
//!
//! Every action is a registered command (`handles`); [`execute`] does the work without a window,
//! [`run`] is what menus and shortcuts call (it opens the window or asks for a file first and
//! then calls [`execute`]).

use std::path::{Path, PathBuf};

use darkburn_core::{CutSetting, ImageAdjust, ShapeId, ShapeKind};
use darkburn_raster::{AdjustPreset, Adjustments, ImageMode, ImageSettings, PresetLibrary};
use darkburn_ui::ui_kit;
use egui::{Align2, Color32, Context, Id, Rect, Sense, Stroke, TextureHandle, TextureOptions, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_theme::Tokens;

use crate::image_ops::{self, AdjustArgs, ConvertArgs, MODE_NAMES, TraceArgs};
use crate::image_render;
use crate::images;
use crate::numeric::{self, Num};
use crate::scan_offsets::{self, ScanOffsetTable};
use crate::shell::{APP_BAR_HEIGHT, DarkBurn};

/// Dots on a side of the Adjust Image preview.
const PREVIEW_SIDE: u32 = 280;
/// Pixels the trace preview works on.
pub(crate) const TRACE_PREVIEW_PIXELS: u64 = 400_000;
const PRESET_FILE: &str = "image_presets.json";

/// Everything the image windows keep between frames.
#[derive(Default)]
pub struct ImageUi {
    pub adjust: Option<AdjustDialog>,
    pub trace: Option<TraceDialog>,
    pub convert: Option<ConvertDialog>,
    /// Trace Image Files [IMG-21].
    pub multi: Option<crate::multi_trace::MultiTraceDialog>,
    /// Import 3D Model [IMG-25].
    pub depth: crate::model_import::DepthUi,
    pub offsets_open: bool,
    presets: Option<PresetLibrary>,
    /// Said under the open window (or in the status bar when none is open).
    pub message: Option<String>,
}

impl ImageUi {
    /// The saved presets (read from the settings folder on first use).
    pub fn presets(&mut self, dir: Option<&Path>) -> &mut PresetLibrary {
        self.presets.get_or_insert_with(|| dir.and_then(|d| PresetLibrary::load(&d.join(PRESET_FILE)).ok()).unwrap_or_default())
    }
}

// ----- Commands -----

const MENU_COMMANDS: &[&str] = &[
    "tools.adjust_image",
    "tools.trace_image",
    "tools.trace_files",
    "tools.mask_image",
    "edit.convert_bitmap",
    "edit.image_replace",
    "edit.image_replace_fit",
    "edit.image_refresh",
    "edit.image_crop",
    "edit.image_flatten",
    "edit.image_remove_crop",
    "edit.image_remove_backup",
    "file.save_bitmap",
];

/// Whether `id` is an image command.
pub fn handles(id: &str) -> bool {
    id.starts_with("image.") || MENU_COMMANDS.contains(&id)
}

/// Whether the command has something to act on with the current selection.
pub fn can_run(app: &DarkBurn, id: &str) -> bool {
    let e = &app.editor;
    let images = image_ops::selected_images(e).len();
    let others = e.selected().len() - images;
    match id {
        "edit.image_crop" | "tools.mask_image" => images == 1 && others > 0,
        "edit.convert_bitmap" | "image.convert_bitmap" => others > 0,
        "tools.adjust_image"
        | "tools.trace_image"
        | "edit.image_replace"
        | "edit.image_replace_fit"
        | "edit.image_refresh"
        | "edit.image_flatten"
        | "edit.image_remove_crop"
        | "edit.image_remove_backup"
        | "file.save_bitmap"
        | "image.adjust"
        | "image.trace"
        | "image.replace"
        | "image.save_bitmap" => images > 0,
        _ => true,
    }
}

fn arg_f64(a: &Value, key: &str) -> Option<f64> {
    a.get(key).and_then(Value::as_f64)
}

fn path_arg(a: &Value) -> Result<PathBuf, String> {
    a.get("path").and_then(Value::as_str).map(PathBuf::from).ok_or_else(|| "argument path must be text".to_string())
}

fn parse<T: serde::de::DeserializeOwned + Default>(a: &Value) -> Result<T, String> {
    if a.is_null() || a.as_object().is_some_and(|o| o.is_empty()) {
        return Ok(T::default());
    }
    serde_json::from_value(a.clone()).map_err(|e| format!("arguments: {e}"))
}

fn ids_of(app: &DarkBurn) -> Vec<ShapeId> {
    image_ops::selected_images(&app.editor)
}

fn changed(done: bool) -> Result<Value, String> {
    Ok(json!({ "changed": done }))
}

/// Runs image command `id` with `args`, as the API and the menus do.
pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Result<Value, String> {
    let dir = app.settings_dir.clone();
    let ids = ids_of(app);
    match id {
        "tools.adjust_image" => open_adjust(app).map(|()| json!({ "opened": "adjust_image" })),
        "tools.trace_image" => open_trace(app).map(|()| json!({ "opened": "trace_image" })),
        "edit.convert_bitmap" => open_convert(app).map(|()| json!({ "opened": "convert_bitmap" })),
        "image.adjust" => {
            let a: AdjustArgs = parse(args)?;
            changed(image_ops::adjust(&mut app.editor, &ids, &a)?)
        }
        "image.trace" => {
            let a: TraceArgs = parse(args)?;
            let r = image_ops::trace(&mut app.editor, &ids, &a)?;
            Ok(json!({ "created": r.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "nodes": r.nodes }))
        }
        "image.trace_capture" => trace_capture(app, args),
        "tools.trace_files" => crate::multi_trace::open(app, TraceArgs::default()).map(|()| json!({ "opened": "trace_files" })),
        "image.trace_files" => crate::multi_trace::execute(app, args),
        "image.depth_map" => crate::model_import::execute(app, args),
        "image.convert_bitmap" => {
            let a: ConvertArgs = parse(args)?;
            let ids: Vec<ShapeId> = app.editor.selected().into_iter().filter(|i| app.editor.doc.get(*i).is_some_and(|s| !s.is_image())).collect();
            let new = image_ops::convert_to_bitmap(&mut app.editor, &ids, &a)?;
            Ok(json!({ "created": new.0 }))
        }
        "tools.mask_image" => {
            let sel = app.editor.selected();
            image_ops::mask_image(&mut app.editor, &sel).map(|i| json!({ "image": i.0 }))
        }
        "edit.image_crop" => {
            let sel = app.editor.selected();
            image_ops::apply_crop_path(&mut app.editor, &sel).map(|i| json!({ "image": i.0 }))
        }
        "edit.image_flatten" => image_ops::flatten_crop(&mut app.editor, &ids).map(|n| json!({ "flattened": n })),
        "edit.image_remove_crop" => image_ops::remove_crop(&mut app.editor, &ids).map(|n| json!({ "removed": n })),
        "edit.image_remove_backup" => image_ops::remove_backup(&mut app.editor, &ids).map(|n| json!({ "removed": n })),
        "edit.image_refresh" => {
            let ids = ids_of(app);
            let mut errors = Vec::new();
            let mut done = 0;
            for i in ids {
                match image_ops::refresh_image(&mut app.editor, i) {
                    Ok(()) => done += 1,
                    Err(e) => errors.push(e),
                }
            }
            if done == 0 && !errors.is_empty() { Err(errors.join("; ")) } else { Ok(json!({ "refreshed": done, "errors": errors })) }
        }
        "edit.image_replace" | "edit.image_replace_fit" | "image.replace" => {
            let fit = id == "edit.image_replace_fit" || args.get("fit").and_then(Value::as_bool).unwrap_or(false);
            let path = match args.get("path") {
                Some(_) => path_arg(args)?,
                None => pick_image().ok_or("no file was chosen")?,
            };
            let bytes = std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let ids = ids_of(app);
            let first = *ids.first().ok_or("select an image first")?;
            image_ops::replace_image(&mut app.editor, first, &bytes, Some(&path), fit)?;
            Ok(json!({ "replaced": first.0 }))
        }
        "file.save_bitmap" | "image.save_bitmap" => {
            let first = *ids_of(app).first().ok_or("select an image first")?;
            let path = match args.get("path") {
                Some(_) => path_arg(args)?,
                None => {
                    rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("processed.png").save_file().ok_or("no file was chosen")?
                }
            };
            image_ops::save_processed_bitmap(&app.editor.doc, first, &path)?;
            Ok(json!({ "saved": path.display().to_string() }))
        }
        "image.presets" => Ok(json!({ "presets": app.images.presets(dir.as_deref()).presets.iter().map(|p| &p.name).collect::<Vec<_>>() })),
        "image.preset_save" => {
            let name =
                args.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).ok_or("argument name must be text")?.trim().to_string();
            let first = *ids_of(app).first().ok_or("select an image first")?;
            let a = current_args(app, first)?;
            save_preset(app, a.to_preset(&name))?;
            Ok(json!({ "saved": name }))
        }
        "image.preset_load" => {
            let name = args.get("name").and_then(Value::as_str).ok_or("argument name must be text")?;
            let p = app.images.presets(dir.as_deref()).get(name).cloned().ok_or_else(|| format!("there is no preset called {name}"))?;
            changed(image_ops::adjust(&mut app.editor, &ids, &AdjustArgs::from_preset(&p))?)
        }
        "image.preset_delete" => {
            let name = args.get("name").and_then(Value::as_str).ok_or("argument name must be text")?;
            let gone = app.images.presets(dir.as_deref()).remove(name);
            persist_presets(app)?;
            Ok(json!({ "deleted": gone }))
        }
        "image.preset_import" => {
            let lib = PresetLibrary::load(&path_arg(args)?).map_err(|e| e.to_string())?;
            let names = app.images.presets(dir.as_deref()).import(lib);
            persist_presets(app)?;
            Ok(json!({ "imported": names }))
        }
        "image.preset_export" => {
            let path = path_arg(args)?;
            let all: Vec<String> = app.images.presets(dir.as_deref()).presets.iter().map(|p| p.name.clone()).collect();
            let names: Vec<String> = match args.get("names").and_then(Value::as_array) {
                Some(l) => l.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                None => all,
            };
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            app.images.presets(dir.as_deref()).export(&refs).save(&path).map_err(|e| e.to_string())?;
            Ok(json!({ "exported": names }))
        }
        "image.scan_offsets" => scan_offsets_command(app, args),
        "image.scan_offsets_import" => {
            let t = ScanOffsetTable::load(&path_arg(args)?)?;
            let mut tuning = scan_offsets::current();
            tuning.offsets = t;
            store_tuning(app, tuning);
            Ok(json!({ "rows": scan_offsets::current().offsets.entries.len() }))
        }
        "image.scan_offsets_export" => {
            scan_offsets::current().offsets.save(&path_arg(args)?)?;
            Ok(json!({ "rows": scan_offsets::current().offsets.entries.len() }))
        }
        other => Err(format!("{other} is not an image command")),
    }
}

/// What menus, buttons and shortcuts call: runs command `id`, saying any problem in the
/// status bar. Returns whether `id` is an image command.
pub fn run(app: &mut DarkBurn, _ctx: &Context, id: &str) -> bool {
    if !handles(id) {
        return false;
    }
    if let Err(e) = execute(app, id, &Value::Null) {
        app.note = Some(e);
    }
    true
}

fn scan_offsets_command(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let mut t = scan_offsets::current();
    let mut touched = false;
    if let (Some(speed), Some(offset)) = (arg_f64(a, "speed"), arg_f64(a, "offset")) {
        t.offsets.set(speed, offset);
        touched = true;
    }
    if let Some(speed) = arg_f64(a, "remove_speed") {
        t.offsets.entries.retain(|e| (e.speed - speed).abs() > 1e-9);
        touched = true;
    }
    if let Some(b) = a.get("fast_skip").and_then(Value::as_bool) {
        t.fast_skip = b;
        touched = true;
    }
    if let Some(mm) = arg_f64(a, "skip_mm") {
        t.skip_mm = mm.clamp(0.1, 1000.0);
        touched = true;
    }
    if touched {
        store_tuning(app, t);
    } else {
        app.images.offsets_open = true;
    }
    let now = scan_offsets::current();
    Ok(
        json!({ "fast_skip": now.fast_skip, "skip_mm": now.skip_mm, "offsets": now.offsets.entries.iter().map(|e| json!([e.speed, e.offset])).collect::<Vec<_>>() }),
    )
}

fn store_tuning(app: &DarkBurn, t: scan_offsets::ScanTuning) {
    if let Some(dir) = &app.settings_dir
        && let Err(e) = t.save_to(dir)
    {
        log::warn!("{e}");
    }
    scan_offsets::set(t);
}

/// Loads the saved scan tuning from the settings folder (once, at start-up).
pub fn load_tuning(app: &DarkBurn) {
    if let Some(dir) = &app.settings_dir {
        scan_offsets::set(scan_offsets::ScanTuning::load_from(dir));
    }
}

fn trace_capture(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let path = path_arg(a)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let capture = images::decode(&bytes)?;
    let (pw, ph) = (f64::from(capture.width()), f64::from(capture.height()));
    let (x, y) = (arg_f64(a, "x").unwrap_or(0.0), arg_f64(a, "y").unwrap_or(0.0));
    let dpi = arg_f64(a, "dpi").filter(|d| *d > 0.0).unwrap_or(254.0);
    let (w, h) = (arg_f64(a, "width").unwrap_or(pw * 25.4 / dpi), arg_f64(a, "height").unwrap_or(ph * 25.4 / dpi));
    let to_mm = darkburn_core::kurbo::Affine::new([w / pw, 0.0, 0.0, -h / ph, x, y + h]);
    let t: TraceArgs = parse(a)?;
    let r = image_ops::trace_capture(&mut app.editor, &capture, to_mm, &t)?;
    Ok(json!({ "created": r.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "nodes": r.nodes }))
}

fn pick_image() -> Option<PathBuf> {
    rfd::FileDialog::new().add_filter("Images", crate::import::BITMAP_EXTENSIONS).pick_file()
}

fn current_args(app: &DarkBurn, id: ShapeId) -> Result<AdjustArgs, String> {
    let s = app.editor.doc.get(id).ok_or("that shape no longer exists")?;
    let ShapeKind::Image(img) = &s.kind else { return Err("select an image first".into()) };
    Ok(AdjustArgs::from_values(&img.adjust, app.editor.doc.layer(s.layer).main()))
}

fn persist_presets(app: &mut DarkBurn) -> Result<(), String> {
    let dir = app.settings_dir.clone();
    let Some(dir) = dir else { return Ok(()) };
    let _ = std::fs::create_dir_all(&dir);
    app.images.presets(Some(&dir)).save(&dir.join(PRESET_FILE)).map_err(|e| e.to_string())
}

fn save_preset(app: &mut DarkBurn, p: AdjustPreset) -> Result<(), String> {
    let dir = app.settings_dir.clone();
    app.images.presets(dir.as_deref()).upsert(p);
    persist_presets(app)
}

// ----- Adjust Image window -----

/// The open Adjust Image window.
pub struct AdjustDialog {
    pub ids: Vec<ShapeId>,
    pub args: AdjustArgs,
    /// What the document shows now (edits apply live).
    applied: AdjustArgs,
    /// The images' adjustments and their layers' settings as they were (Cancel puts them back).
    original: (Vec<(ShapeId, ImageAdjust)>, Vec<(usize, CutSetting)>),
    pub preset_name: String,
    preview: Option<(String, TextureHandle, TextureHandle, bool)>,
    pub warning: Option<String>,
    pub notice: Option<String>,
    /// The project had no unsaved changes when the window opened: the number of undo steps then.
    clean_at_open: Option<usize>,
    /// A live edit was written to the document.
    touched: bool,
    /// The project was saved while live edits were in it.
    saved_since: bool,
}

impl AdjustDialog {
    /// The window for `ids`, starting from the first image's values.
    pub fn new(app: &DarkBurn, ids: Vec<ShapeId>) -> Result<AdjustDialog, String> {
        let first = *ids.first().ok_or("select an image first")?;
        let args = current_args(app, first)?;
        let doc = &app.editor.doc;
        let mut adjusts = Vec::new();
        let mut layers = Vec::new();
        for id in &ids {
            if let Some(s) = doc.get(*id)
                && let ShapeKind::Image(i) = &s.kind
            {
                adjusts.push((*id, i.adjust));
                if !layers.iter().any(|(l, _)| *l == s.layer) {
                    layers.push((s.layer, doc.layer(s.layer).main().clone()));
                }
            }
        }
        Ok(AdjustDialog {
            ids,
            applied: args.clone(),
            args,
            original: (adjusts, layers),
            preset_name: String::new(),
            preview: None,
            warning: None,
            notice: None,
            clean_at_open: (!app.editor.is_modified()).then(|| app.editor.history.len()),
            touched: false,
            saved_since: false,
        })
    }

    /// Writes edits made in the window to the document (no undo step).
    pub fn sync(&mut self, app: &mut DarkBurn) {
        if self.touched && !app.editor.is_modified() {
            self.saved_since = true;
        }
        if self.args != self.applied {
            match image_ops::adjust_doc(&mut app.editor.doc, &self.ids, &self.args) {
                Ok(_) => {
                    app.editor.touch();
                    self.touched = true;
                    self.applied = self.args.clone();
                }
                Err(e) => self.notice = Some(e),
            }
        }
        self.warning = self.memory_warning(app);
    }

    fn memory_warning(&self, app: &DarkBurn) -> Option<String> {
        let doc = &app.editor.doc;
        self.ids
            .iter()
            .filter_map(|id| doc.get(*id))
            .filter_map(|s| match &s.kind {
                ShapeKind::Image(i) => {
                    let (w, h) = images::size_mm(s, i);
                    images::memory_warning(images::dots_for(w, h, doc.layer(s.layer).main().interval))
                }
                _ => None,
            })
            .next()
    }

    /// Puts the document back as it was when the window opened. A project without unsaved
    /// changes then has none again (unless it was saved with the live edits in it).
    pub fn restore(&self, app: &mut DarkBurn) {
        if !self.touched {
            // Nothing was written, so there is nothing to put back and nothing has changed.
            return;
        }
        let saved_since = self.saved_since || !app.editor.is_modified();
        let doc = &mut app.editor.doc;
        for (id, adj) in &self.original.0 {
            if let Some(s) = doc.get_mut(*id)
                && let ShapeKind::Image(i) = &mut s.kind
            {
                i.adjust = *adj;
            }
        }
        for (l, c) in &self.original.1 {
            *doc.layer_mut(*l).main_mut() = c.clone();
        }
        app.editor.touch();
        // (Not when something was undone or done meanwhile: the document is then another one.)
        if self.clean_at_open == Some(app.editor.history.len()) && !saved_since {
            app.editor.mark_saved();
        }
    }

    /// OK: the changes become one undo step.
    pub fn commit(&self, app: &mut DarkBurn) {
        self.restore(app);
        if let Err(e) = image_ops::adjust(&mut app.editor, &self.ids, &self.args) {
            app.note = Some(e);
        }
    }

    fn apply_preset(&mut self, p: &AdjustPreset) {
        self.args = AdjustArgs::from_preset(p);
    }

    /// The side-by-side pictures: the untouched picture and the burn, same part of the image.
    fn refresh_preview(&mut self, ctx: &Context, app: &DarkBurn) {
        let doc = &app.editor.doc;
        let Some(leaf) = self.ids.first().and_then(|id| doc.get(*id)) else { return };
        let ShapeKind::Image(img) = &leaf.kind else { return };
        let layer = doc.layer(leaf.layer).main();
        let adjust = images::adjustments(&img.adjust);
        let settings = images::image_settings(layer, 0.0);
        let key = format!("{}{:?}{:?}{:?}", img.image, adjust, settings, images::size_mm(leaf, img));
        if self.preview.as_ref().is_some_and(|p| p.0 == key) {
            return;
        }
        let Ok(gray) = images::source_gray(doc, img) else { return };
        let (sub, size, cropped) = image_render::window(&gray, images::size_mm(leaf, img), settings.line_interval_mm, PREVIEW_SIDE);
        let plain = ImageSettings { mode: ImageMode::Grayscale, ..settings.clone() };
        let before = image_render::render(&sub, size, &Adjustments::default(), &plain, PREVIEW_SIDE);
        let after = image_render::render(&sub, size, &adjust, &settings, PREVIEW_SIDE);
        let opts = TextureOptions::NEAREST;
        let b = ctx.load_texture("db_adjust_before", before.image, opts);
        let a = ctx.load_texture("db_adjust_after", after.image, opts);
        self.preview = Some((key, b, a, cropped));
    }
}

fn open_adjust(app: &mut DarkBurn) -> Result<(), String> {
    let ids = image_ops::selected_images(&app.editor);
    app.images.adjust = Some(AdjustDialog::new(app, ids)?);
    Ok(())
}

fn num_field(ui: &mut Ui, label: &str, v: &mut Option<f64>, n: Num) {
    let mut x = v.unwrap_or(0.0);
    if ui.push_id(label, |ui| numeric::row(ui, label, &mut x, &n)).inner.changed {
        *v = Some(x);
    }
}

fn mode_picker(ui: &mut Ui, label: &str, id: &str, name: &mut Option<String>) {
    let labels: Vec<&str> = MODE_NAMES.iter().map(|(_, m)| m.label()).collect();
    let current = name.as_deref().and_then(|n| MODE_NAMES.iter().position(|(k, _)| *k == n)).unwrap_or(3);
    let mut i = current;
    ui_kit::choice_row(ui, label, id, &mut i, &labels);
    if i != current {
        *name = Some(MODE_NAMES[i].0.to_string());
    }
}

fn flag(ui: &mut Ui, label: &str, v: &mut Option<bool>) {
    let mut b = v.unwrap_or(false);
    if ui_kit::checkbox(ui, label, &mut b).changed() {
        *v = Some(b);
    }
}

/// The gap between two columns side by side, with room for a rule down its middle. Returns where
/// the rule goes; [`column_rule`] draws it once the row is laid out and its height is known.
/// Not `ui.separator()`: in a row that takes all the height it is offered, and offered all the
/// height a window may have it made Adjust Image and Trace Image as tall as the main window,
/// whatever was in them.
pub(crate) fn column_gap(ui: &mut Ui) -> f32 {
    // As wide as egui's own separator.
    let (gap, _) = ui.allocate_exact_size(vec2(6.0, 0.0), Sense::hover());
    gap.center().x
}

/// The rule in a [`column_gap`], as tall as the row of columns.
pub(crate) fn column_rule(ui: &Ui, x: f32, row: Rect) {
    ui.painter().vline(x, row.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
}

/// The body of Adjust Image or Trace Image, kept short enough for the window to fit under the
/// application bar; what does not fit scrolls. The kit offers a body the height of the main
/// window less the window's own header and footer, and in a small main window a window that
/// tall has its top edge over the bar.
pub(crate) fn under_the_bar(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    let height = (ui.available_height() - APP_BAR_HEIGHT).max(0.0);
    egui::ScrollArea::vertical().id_salt("image_window_body").max_height(height).auto_shrink([false, true]).show(ui, body);
}

fn adjust_body(ui: &mut Ui, app: &mut DarkBurn, d: &mut AdjustDialog) {
    let t = Tokens::get();
    let mut rule = 0.0;
    let row = ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(330.0);
            let a = &mut d.args;
            ui_kit::section_heading(ui, "Tone");
            num_field(ui, "Brightness", &mut a.brightness, Num::new(-100.0..=100.0, 0, " %").speed(0.5));
            num_field(ui, "Contrast", &mut a.contrast, Num::new(-100.0..=100.0, 0, " %").speed(0.5));
            num_field(ui, "Gamma", &mut a.gamma, Num::new(0.1..=10.0, 2, "").speed(0.01));
            ui_kit::section_heading(ui, "Enhance");
            num_field(ui, "Enhance amount", &mut a.enhance_amount, Num::new(0.0..=1000.0, 0, " %").speed(0.5));
            num_field(ui, "Enhance radius", &mut a.enhance_radius, Num::new(0.0..=100.0, 1, " px").speed(0.1));
            num_field(ui, "Denoise", &mut a.enhance_denoise, Num::new(0.0..=100.0, 0, "").speed(0.5));
            ui_kit::section_heading(ui, "Burn");
            mode_picker(ui, "Image mode", "adj_mode", &mut a.image_mode);
            num_field(ui, "DPI", &mut a.dpi, Num::new(1.0..=25_400.0, 1, "").speed(1.0));
            flag(ui, "Negative image", &mut a.negative);
            flag(ui, "Pass-through (already dithered)", &mut a.pass_through);
            if a.image_mode.as_deref() == Some("halftone") {
                num_field(ui, "Halftone cells per inch", &mut a.halftone_cells, Num::new(1.0..=1000.0, 1, "").speed(0.5));
                num_field(ui, "Halftone angle", &mut a.halftone_angle, Num::new(-360.0..=360.0, 1, " deg").speed(0.5));
            }
            if a.image_mode.as_deref() == Some("threshold") {
                num_field(ui, "Ramp length", &mut a.threshold_ramp, Num::new(0.0..=100.0, 2, " mm").speed(0.01));
            }
            if let Some(w) = &d.warning {
                ui_kit::status_line(ui, w, ui_kit::StatusTone::Warning);
            }
            if let Some(n) = &d.notice {
                ui_kit::status_line(ui, n, ui_kit::StatusTone::Error);
            }
            presets_section(ui, app, d);
        });
        rule = column_gap(ui);
        ui.vertical(|ui| {
            let side = PREVIEW_SIDE as f32 * 0.9;
            ui.label(egui::RichText::new("Before and after").size(t.font_small).color(t.text_dim));
            match &d.preview {
                Some((_, before, after, cropped)) => {
                    let aspect = before.size()[0] as f32 / before.size()[1].max(1) as f32;
                    let size = if aspect >= 1.0 { vec2(side, side / aspect) } else { vec2(side * aspect, side) };
                    for (label, tex) in [("Original", before), ("Burn", after)] {
                        ui.label(egui::RichText::new(label).size(t.font_micro).color(t.text_dim));
                        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                        ui.painter().rect_filled(r, 0.0, Color32::WHITE);
                        ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                    }
                    if *cropped {
                        ui.label(egui::RichText::new("The middle of the image, at the real resolution.").size(t.font_micro).color(t.text_dim));
                    }
                }
                None => {
                    ui.label("No preview: the picture cannot be read.");
                }
            }
        });
    });
    column_rule(ui, rule, row.response.rect);
}

fn presets_section(ui: &mut Ui, app: &mut DarkBurn, d: &mut AdjustDialog) {
    ui_kit::section_heading(ui, "Presets");
    let dir = app.settings_dir.clone();
    let names: Vec<String> = app.images.presets(dir.as_deref()).presets.iter().map(|p| p.name.clone()).collect();
    if names.is_empty() {
        ui_kit::status_line(ui, "No presets yet. Type a name and press Save.", ui_kit::StatusTone::Dim);
    }
    ui.horizontal_wrapped(|ui| {
        for n in &names {
            if ui_kit::choice_chip(ui, &format!("preset_{n}"), n, d.preset_name == *n).clicked() {
                d.preset_name = n.clone();
                if let Some(p) = app.images.presets(dir.as_deref()).get(n).cloned() {
                    d.apply_preset(&p);
                }
            }
        }
    });
    ui_kit::row(ui, "Name", |ui| ui_kit::text_field(ui, &mut d.preset_name, "Preset name").1);
    ui.horizontal_wrapped(|ui| {
        let named = !d.preset_name.trim().is_empty();
        if ui.add_enabled_ui(named, |ui| ui_kit::button(ui, "Save", ui_kit::Button::Small)).inner.clicked() {
            let name = d.preset_name.trim().to_string();
            d.notice = save_preset(app, d.args.to_preset(&name)).err();
        }
        if ui.add_enabled_ui(names.contains(&d.preset_name), |ui| ui_kit::button(ui, "Delete", ui_kit::Button::Small)).inner.clicked() {
            app.images.presets(dir.as_deref()).remove(&d.preset_name);
            d.notice = persist_presets(app).err();
        }
        if ui_kit::button(ui, "Import...", ui_kit::Button::Small).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Image presets", &["json"]).pick_file()
        {
            d.notice = execute(app, "image.preset_import", &json!({ "path": p.display().to_string() })).err();
        }
        if ui_kit::button(ui, "Export...", ui_kit::Button::Small).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Image presets", &["json"]).set_file_name("image-presets.json").save_file()
        {
            d.notice = execute(app, "image.preset_export", &json!({ "path": p.display().to_string() })).err();
        }
    });
}

// ----- Trace Image window -----

/// The open Trace Image window.
pub struct TraceDialog {
    pub ids: Vec<ShapeId>,
    pub args: TraceArgs,
    /// Clip to a rectangle, as percent of the picture from its top-left corner: left, top, right, bottom.
    pub clip_on: bool,
    pub clip: [f64; 4],
    base: Option<TextureHandle>,
    /// Paths of the preview, in the image's own millimetres (centred, Y up), and what they were made with.
    paths: Vec<darkburn_core::kurbo::BezPath>,
    paths_for: Option<String>,
    size_mm: (f64, f64),
    pub notice: Option<String>,
    /// The user asked to trace several files with these settings instead [IMG-21].
    pub more_files: bool,
}

impl TraceDialog {
    pub fn new(app: &DarkBurn, ids: Vec<ShapeId>) -> Result<TraceDialog, String> {
        let first = *ids.first().ok_or("select an image first")?;
        let s = app.editor.doc.get(first).ok_or("that shape no longer exists")?;
        let ShapeKind::Image(img) = &s.kind else { return Err("select an image first".into()) };
        Ok(TraceDialog {
            ids,
            args: TraceArgs::default(),
            clip_on: false,
            clip: [0.0, 0.0, 100.0, 100.0],
            base: None,
            paths: Vec::new(),
            paths_for: None,
            size_mm: (img.width, img.height),
            notice: None,
            more_files: false,
        })
    }

    /// The arguments the trace uses (clip rectangle included).
    pub fn effective(&self) -> TraceArgs {
        let mut a = self.args.clone();
        a.clip = self.clip_on.then(|| [self.clip[0] / 100.0, self.clip[1] / 100.0, self.clip[2] / 100.0, self.clip[3] / 100.0]);
        a
    }

    /// What the preview paths depend on (the display options are left out).
    fn key(&self) -> String {
        let a = self.effective();
        format!(
            "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            a.threshold, a.lower_cutoff, a.ignore_smaller, a.smoothness, a.optimize, a.clip, a.sketch, a.sketch_radius, a.sketch_sensitivity
        )
    }

    fn refresh(&mut self, ctx: &Context, app: &DarkBurn) {
        let doc = &app.editor.doc;
        let Some(id) = self.ids.first().copied() else { return };
        let Some(leaf) = doc.get(id) else { return };
        let ShapeKind::Image(img) = &leaf.kind else { return };
        if self.base.is_none()
            && let Ok(gray) = images::source_gray(doc, img)
        {
            let plain =
                ImageSettings { mode: ImageMode::Grayscale, line_interval_mm: (img.width.max(img.height) / 400.0).max(1e-3), ..Default::default() };
            let r = image_render::render(&gray, (img.width, img.height), &images::adjustments(&img.adjust), &plain, 400);
            self.base = Some(ctx.load_texture("db_trace_base", r.image, TextureOptions::LINEAR));
        }
        let key = self.key();
        if self.paths_for.as_ref() == Some(&key) {
            return;
        }
        self.paths_for = Some(key);
        match image_ops::trace_paths_limited(doc, id, &self.effective(), TRACE_PREVIEW_PIXELS) {
            Ok(mut paths) => {
                // Into the picture's own frame so the preview can draw it without the shape's placement.
                if leaf.transform.determinant().abs() > 1e-12 {
                    let inv = leaf.transform.inverse();
                    paths.iter_mut().for_each(|p| p.apply_affine(inv));
                }
                self.paths = paths;
                self.size_mm = (img.width, img.height);
                self.notice = None;
            }
            Err(e) => self.notice = Some(e),
        }
    }
}

fn open_trace(app: &mut DarkBurn) -> Result<(), String> {
    let ids = image_ops::selected_images(&app.editor);
    app.images.trace = Some(TraceDialog::new(app, ids)?);
    Ok(())
}

fn trace_body(ui: &mut Ui, d: &mut TraceDialog) {
    let mut rule = 0.0;
    let row = ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(320.0);
            ui_kit::section_heading(ui, "Trace");
            trace_settings(ui, &mut d.args);
            ui_kit::section_heading(ui, "Area");
            ui_kit::checkbox(ui, "Trace only inside a rectangle", &mut d.clip_on);
            if d.clip_on {
                for (i, label) in ["Left", "Top", "Right", "Bottom"].into_iter().enumerate() {
                    numeric::row(ui, label, &mut d.clip[i], &Num::new(0.0..=100.0, 1, " %").speed(0.2));
                }
            }
            ui_kit::section_heading(ui, "Preview");
            let mut fade = f64::from(d.args.fade);
            if numeric::row(ui, "Fade image", &mut fade, &Num::new(0.0..=1.0, 2, "").speed(0.01)).changed {
                d.args.fade = fade as f32;
            }
            ui_kit::checkbox(ui, "Show points", &mut d.args.show_points);
            ui_kit::checkbox(ui, "Delete image after tracing", &mut d.args.delete_image);
            ui_kit::caption(ui, "Cutoff", &format!("{} outlines, {} nodes", d.paths.len(), image_ops::node_count(&d.paths)));
            if ui_kit::button(ui, "Trace several files...", ui_kit::Button::Small).clicked() {
                d.more_files = true;
            }
            if let Some(n) = &d.notice {
                ui_kit::status_line(ui, n, ui_kit::StatusTone::Error);
            }
        });
        rule = column_gap(ui);
        ui.vertical(|ui| {
            let clip = d.clip_on.then_some(d.clip);
            trace_preview(ui, d.base.as_ref(), d.size_mm, &d.paths, (d.args.fade, d.args.show_points), clip);
        });
    });
    column_rule(ui, rule, row.response.rect);
}

/// The trace settings rows (cutoffs, speckle size, smoothing, sketch), shared with Trace
/// Image Files.
pub(crate) fn trace_settings(ui: &mut Ui, a: &mut TraceArgs) {
    let mut th = f64::from(a.threshold);
    if numeric::row(ui, "Cutoff", &mut th, &Num::new(0.0..=255.0, 0, "").speed(0.5)).changed {
        a.threshold = th.clamp(0.0, 255.0) as u8;
    }
    let mut lo = f64::from(a.lower_cutoff);
    if numeric::row(ui, "Lower cutoff", &mut lo, &Num::new(0.0..=255.0, 0, "").speed(0.5)).changed {
        a.lower_cutoff = lo.clamp(0.0, 255.0) as u8;
    }
    let mut ign = f64::from(a.ignore_smaller);
    if numeric::row(ui, "Ignore less than", &mut ign, &Num::new(0.0..=10_000.0, 0, " px").speed(0.5)).changed {
        a.ignore_smaller = ign.clamp(0.0, 10_000.0) as u32;
    }
    let mut sm = f64::from(a.smoothness);
    if numeric::row(ui, "Smoothness", &mut sm, &Num::new(0.0..=2.0, 2, "").speed(0.01)).changed {
        a.smoothness = sm as f32;
    }
    let mut op = f64::from(a.optimize);
    if numeric::row(ui, "Optimize", &mut op, &Num::new(0.05..=20.0, 2, "").speed(0.02)).changed {
        a.optimize = op as f32;
    }
    ui_kit::checkbox(ui, "Sketch trace (lines instead of outlines)", &mut a.sketch);
    if a.sketch {
        let mut r = f64::from(a.sketch_radius);
        if numeric::row(ui, "Detail size", &mut r, &Num::new(0.5..=50.0, 1, " px").speed(0.1)).changed {
            a.sketch_radius = r as f32;
        }
        let mut s = f64::from(a.sketch_sensitivity);
        if numeric::row(ui, "Sensitivity", &mut s, &Num::new(0.0..=1.0, 2, "").speed(0.01)).changed {
            a.sketch_sensitivity = s as f32;
        }
    }
}

/// The trace preview: the picture (`base`, faded by `fade`) with `paths` over it, in the
/// picture's own millimetres (centred, Y up) of `size_mm`; `clip` is a rectangle in percent.
pub(crate) fn trace_preview(
    ui: &mut Ui,
    base: Option<&TextureHandle>,
    size_mm: (f64, f64),
    paths: &[darkburn_core::kurbo::BezPath],
    (fade, show_points): (f32, bool),
    clip: Option<[f64; 4]>,
) {
    let t = Tokens::get();
    let side = 340.0f32;
    let (w, h) = size_mm;
    let aspect = (w / h.max(1e-9)) as f32;
    let size = if aspect >= 1.0 { vec2(side, side / aspect) } else { vec2(side * aspect, side) };
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, 0.0, Color32::WHITE);
    if let Some(tex) = base {
        let fade = fade.clamp(0.0, 1.0);
        p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE.gamma_multiply(1.0 - fade));
    }
    let to_screen = |pt: darkburn_core::kurbo::Point| {
        pos2(r.left() + ((pt.x + w / 2.0) / w) as f32 * r.width(), r.top() + ((h / 2.0 - pt.y) / h) as f32 * r.height())
    };
    let ink = t.accent;
    let mut points = 0;
    for path in paths {
        let mut line: Vec<egui::Pos2> = Vec::new();
        let flush = |line: &mut Vec<egui::Pos2>| {
            if line.len() > 1 {
                p.add(egui::Shape::line(std::mem::take(line), Stroke::new(1.5, ink)));
            }
            line.clear();
        };
        darkburn_core::kurbo::flatten(path.elements().iter().copied(), 0.05 * w.max(h) / 100.0, |el| match el {
            darkburn_core::kurbo::PathEl::MoveTo(pt) => {
                flush(&mut line);
                line.push(to_screen(pt));
            }
            darkburn_core::kurbo::PathEl::LineTo(pt) => line.push(to_screen(pt)),
            darkburn_core::kurbo::PathEl::ClosePath => {
                if let Some(first) = line.first().copied() {
                    line.push(first);
                }
                flush(&mut line);
            }
            _ => {}
        });
        flush(&mut line);
        if show_points {
            for el in path.elements() {
                let pt = match el {
                    darkburn_core::kurbo::PathEl::MoveTo(pt) | darkburn_core::kurbo::PathEl::LineTo(pt) => *pt,
                    darkburn_core::kurbo::PathEl::QuadTo(_, pt) | darkburn_core::kurbo::PathEl::CurveTo(_, _, pt) => *pt,
                    darkburn_core::kurbo::PathEl::ClosePath => continue,
                };
                if points < 6000 {
                    p.circle_filled(to_screen(pt), 2.0, t.warning);
                    points += 1;
                }
            }
        }
    }
    if let Some(c) = clip {
        let cr = Rect::from_min_max(
            pos2(r.left() + (c[0] / 100.0) as f32 * r.width(), r.top() + (c[1] / 100.0) as f32 * r.height()),
            pos2(r.left() + (c[2] / 100.0) as f32 * r.width(), r.top() + (c[3] / 100.0) as f32 * r.height()),
        );
        p.rect_stroke(cr, 0.0, Stroke::new(1.0, t.warning), egui::StrokeKind::Middle);
    }
    ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
}

// ----- Convert to Bitmap window -----

/// The open Convert to Bitmap window.
pub struct ConvertDialog {
    pub ids: Vec<ShapeId>,
    pub args: ConvertArgs,
    pub outline: bool,
    pub notice: Option<String>,
    /// Size of the result in pixels, for the readout.
    pub pixels: Option<(u64, u64)>,
}

impl ConvertDialog {
    pub fn new(app: &DarkBurn, ids: Vec<ShapeId>) -> Result<ConvertDialog, String> {
        if ids.is_empty() {
            return Err("select shapes to convert first".into());
        }
        let closed = ids.iter().filter_map(|i| app.editor.doc.get(*i)).all(|s| s.is_closed());
        let mut d = ConvertDialog { ids, args: ConvertArgs::default(), outline: !closed, notice: None, pixels: None };
        d.update(app);
        Ok(d)
    }

    /// The arguments with the fill/outline choice made.
    pub fn effective(&self) -> ConvertArgs {
        ConvertArgs { fill: Some(!self.outline), ..self.args.clone() }
    }

    fn update(&mut self, app: &DarkBurn) {
        let (mut w, mut h) = (0.0, 0.0);
        if let Some(b) = app.editor.doc.bbox_of(&self.ids) {
            (w, h) = (b.width(), b.height());
        }
        let k = self.args.dpi.max(1.0) / 25.4;
        self.pixels = Some(((w * k).ceil() as u64, (h * k).ceil() as u64));
    }

    /// Why the result would be refused, if so.
    pub fn problem(&self) -> Option<String> {
        let (w, h) = self.pixels?;
        (w * h > image_ops::CONVERT_MAX_PIXELS).then(|| format!("{w} x {h} pixels is too large: lower the DPI"))
    }
}

fn open_convert(app: &mut DarkBurn) -> Result<(), String> {
    let ids: Vec<ShapeId> = app.editor.selected().into_iter().filter(|i| app.editor.doc.get(*i).is_some_and(|s| !s.is_image())).collect();
    app.images.convert = Some(ConvertDialog::new(app, ids)?);
    Ok(())
}

fn convert_body(ui: &mut Ui, d: &mut ConvertDialog) -> bool {
    let mut changed = false;
    changed |= numeric::row(ui, "DPI", &mut d.args.dpi, &Num::new(5.0..=4800.0, 0, "").speed(1.0)).changed;
    let mut i = usize::from(d.outline);
    ui_kit::choice_row(ui, "Draw", "convert_draw", &mut i, &["Filled shapes", "Outlines"]);
    if (i == 1) != d.outline {
        d.outline = i == 1;
    }
    if d.outline {
        numeric::row(ui, "Pen width", &mut d.args.stroke_mm, &Num::new(0.01..=10.0, 2, " mm").speed(0.01));
    }
    ui_kit::checkbox(ui, "Keep the original shapes", &mut d.args.keep_original);
    if let Some((w, h)) = d.pixels {
        ui_kit::caption(ui, "DPI", &format!("The bitmap will be {w} x {h} pixels."));
    }
    if let Some(p) = d.problem() {
        ui_kit::status_line(ui, &p, ui_kit::StatusTone::Warning);
    }
    if let Some(n) = &d.notice {
        ui_kit::status_line(ui, n, ui_kit::StatusTone::Error);
    }
    changed
}

// ----- Windows -----

/// Draws the open image windows; called once a frame.
pub fn dialogs(ctx: &Context, app: &mut DarkBurn) {
    if let Some(mut d) = app.images.adjust.take() {
        d.sync(app);
        d.refresh_preview(ctx, app);
        let (closed, action) = ui_kit::Window::new(Id::unique("db_adjust_image"), "settings", "Adjust Image")
            .cards(2.9)
            .fields()
            // In the middle of the room under the application bar. Where the kit puts a window
            // that is given no place, a tall one reaches over the bar in a small main window.
            .placed(Align2::CENTER_CENTER, crate::shell::room_under_the_bar(ctx).center())
            .actions("OK", "Cancel", Some("Apply"))
            .show(ctx, true, |ui| under_the_bar(ui, |ui| adjust_body(ui, app, &mut d)));
        d.sync(app);
        match (closed, action) {
            (_, ui_kit::Action::Ok) => d.commit(app),
            (_, ui_kit::Action::Apply) => {
                d.commit(app);
                if let Ok(next) = AdjustDialog::new(app, d.ids.clone()) {
                    app.images.adjust = Some(next);
                }
            }
            (true, _) | (_, ui_kit::Action::Cancel) => d.restore(app),
            _ => app.images.adjust = Some(d),
        }
    }
    if let Some(mut d) = app.images.trace.take() {
        d.refresh(ctx, app);
        let (closed, action) = ui_kit::Window::new(Id::unique("db_trace_image"), "trace", "Trace Image")
            .cards(2.9)
            .fields()
            .placed(Align2::CENTER_CENTER, crate::shell::room_under_the_bar(ctx).center())
            .actions("OK", "Cancel", None)
            .show(ctx, true, |ui| under_the_bar(ui, |ui| trace_body(ui, &mut d)));
        match (closed, action) {
            (_, ui_kit::Action::Ok) => {
                if let Err(e) = image_ops::trace(&mut app.editor, &d.ids, &d.effective()) {
                    d.notice = Some(e.clone());
                    app.images.trace = Some(d);
                }
            }
            (true, _) | (_, ui_kit::Action::Cancel) => {}
            _ if d.more_files => {
                // The same settings, for files picked now; this window gives way to that one.
                let mut args = d.effective();
                args.clip = None;
                if let Err(e) = crate::multi_trace::open(app, args) {
                    d.more_files = false;
                    d.notice = Some(e);
                    app.images.trace = Some(d);
                }
            }
            _ => app.images.trace = Some(d),
        }
    }
    crate::multi_trace::dialog(ctx, app);
    crate::model_import::dialog(ctx, app);
    if let Some(mut d) = app.images.convert.take() {
        let mut edited = false;
        let (closed, action) = ui_kit::Window::new(Id::unique("db_convert_bitmap"), "db_app", "Convert to Bitmap")
            .fields()
            .actions("Convert", "Cancel", None)
            .show(ctx, true, |ui| edited = convert_body(ui, &mut d));
        if edited {
            d.update(app);
        }
        match (closed, action) {
            (_, ui_kit::Action::Ok) => match image_ops::convert_to_bitmap(&mut app.editor, &d.ids, &d.effective()) {
                Ok(_) => {}
                Err(e) => {
                    d.notice = Some(e);
                    app.images.convert = Some(d);
                }
            },
            (true, _) | (_, ui_kit::Action::Cancel) => {}
            _ => app.images.convert = Some(d),
        }
    }
    if app.images.offsets_open {
        offsets_window(ctx, app);
    }
}

fn offsets_window(ctx: &Context, app: &mut DarkBurn) {
    let mut t = scan_offsets::current();
    let before = t.clone();
    let mut pick: Option<bool> = None;
    let (closed, action) =
        ui_kit::Window::new(Id::unique("db_scan_offsets"), "settings", "Scanning Offset").fields().actions("", "Close", None).show(ctx, true, |ui| {
            ui_kit::section_heading(ui, "Blank areas");
            ui_kit::checkbox(ui, "Fast skip of blank areas", &mut t.fast_skip);
            if t.fast_skip {
                numeric::row(ui, "Skip gaps longer than", &mut t.skip_mm, &Num::new(0.1..=1000.0, 1, " mm").speed(0.1));
            }
            ui_kit::section_heading(ui, "Scanning offset by speed");
            ui_kit::caption(ui, "Speed", "Bi-directional lines are moved apart by this much at the speed listed.");
            let mut remove = None;
            for (i, e) in t.offsets.entries.iter_mut().enumerate() {
                ui.push_id(("so_row", i), |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized(vec2(110.0, 24.0), |ui: &mut Ui| {
                            numeric::field(ui, &mut e.speed, &Num::new(0.1..=100_000.0, 0, " mm/s").speed(1.0)).response
                        });
                        ui.add_sized(vec2(110.0, 24.0), |ui: &mut Ui| {
                            numeric::field(ui, &mut e.offset, &Num::new(-10.0..=10.0, 3, " mm").speed(0.005)).response
                        });
                        if ui_kit::button(ui, "Remove", ui_kit::Button::Small).clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                t.offsets.remove(i);
            }
            ui.horizontal(|ui| {
                if t.offsets.entries.len() < scan_offsets::MAX_ROWS && ui_kit::button(ui, "Add row", ui_kit::Button::Small).clicked() {
                    let speed = t.offsets.entries.last().map_or(100.0, |e| e.speed + 100.0);
                    let off = t.offsets.entries.last().map_or(0.0, |e| e.offset);
                    t.offsets.entries.push(scan_offsets::OffsetEntry { speed, offset: off });
                }
                if ui_kit::button(ui, "Import...", ui_kit::Button::Small).clicked() {
                    pick = Some(true);
                }
                if ui_kit::button(ui, "Export...", ui_kit::Button::Small).clicked() {
                    pick = Some(false);
                }
            });
            if let Some(m) = &app.images.message {
                ui_kit::status_line(ui, m, ui_kit::StatusTone::Warning);
            }
        });
    if t != before {
        store_tuning(app, t);
    }
    match pick {
        Some(true) => {
            if let Some(p) = rfd::FileDialog::new().add_filter("Scanning offset", &[scan_offsets::EXTENSION]).pick_file() {
                app.images.message = execute(app, "image.scan_offsets_import", &json!({ "path": p.display().to_string() })).err();
            }
        }
        Some(false) => {
            if let Some(p) =
                rfd::FileDialog::new().add_filter("Scanning offset", &[scan_offsets::EXTENSION]).set_file_name("scan-offsets.dbso").save_file()
            {
                app.images.message = execute(app, "image.scan_offsets_export", &json!({ "path": p.display().to_string() })).err();
            }
        }
        None => {}
    }
    if closed || action != ui_kit::Action::None {
        let mut t = scan_offsets::current();
        t.offsets.normalise();
        store_tuning(app, t);
        app.images.offsets_open = false;
        app.images.message = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::testutil::*;
    use crate::prefs::Prefs;
    use darkburn_core::Shape;
    use darkburn_core::kurbo::Rect as KRect;
    use darkburn_core::layer::ImageMode as LayerMode;

    fn app_with_image() -> (DarkBurn, ShapeId) {
        let mut app = DarkBurn::new(Prefs::default());
        image_layer(&mut app.editor.doc, 3, 0.25);
        let id = add_image(&mut app.editor.doc, png(40, 20, |x, _| if x < 20 { 0 } else { 255 }), (40, 20), (20.0, 10.0), (50.0, 50.0), 3);
        app.editor.selection.set([id]);
        (app, id)
    }

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        crate::api::execute_headless(app, id, &args)
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("darkburn-imgui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn image_commands_run_headless() {
        let (mut app, id) = app_with_image();
        let r = run(&mut app, "image.adjust", json!({ "brightness": 12, "image_mode": "atkinson", "dpi": 150 })).unwrap();
        assert_eq!(r["changed"], true);
        assert_eq!(app.editor.doc.layer(3).main().image_mode, LayerMode::Atkinson);
        let r = run(&mut app, "image.trace", json!({ "layer": 2 })).unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 1);
        assert!(r["nodes"].as_u64().unwrap() > 0);
        app.editor.selection.set([id]);
        let dir = temp("cmds");
        let out = dir.join("burn.png");
        run(&mut app, "image.save_bitmap", json!({ "path": out.display().to_string() })).unwrap();
        assert!(out.is_file());
        assert!(run(&mut app, "image.adjust", json!({ "gamma": "lots" })).is_err());
        assert!(run(&mut app, "image.nope", Value::Null).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commands_need_the_right_selection() {
        let (mut app, id) = app_with_image();
        app.editor.selection.clear();
        for cmd in [
            "tools.adjust_image",
            "tools.trace_image",
            "edit.image_flatten",
            "file.save_bitmap",
            "edit.image_crop",
            "tools.mask_image",
            "edit.convert_bitmap",
        ] {
            assert!(run(&mut app, cmd, Value::Null).is_err(), "{cmd} with nothing selected");
        }
        let shape = app.editor.doc.add(Shape::rect(KRect::new(40.0, 45.0, 50.0, 55.0), 0.0, 1));
        app.editor.selection.set([id, shape]);
        assert!(can_run(&app, "edit.image_crop") && can_run(&app, "tools.mask_image") && can_run(&app, "edit.convert_bitmap"));
        run(&mut app, "edit.image_crop", Value::Null).unwrap();
        assert!(matches!(&app.editor.doc.get(id).unwrap().kind, ShapeKind::Image(i) if i.crop.is_some()));
        run(&mut app, "edit.image_flatten", Value::Null).unwrap();
        assert!(matches!(&app.editor.doc.get(id).unwrap().kind, ShapeKind::Image(i) if i.crop.is_none() && i.backup.is_some()));
        run(&mut app, "edit.image_remove_backup", Value::Null).unwrap();
        assert!(run(&mut app, "edit.image_remove_backup", Value::Null).is_err());
    }

    #[test]
    fn convert_to_bitmap_and_mask_by_command() {
        let (mut app, id) = app_with_image();
        let r = app.editor.doc.add(Shape::rect(KRect::new(0.0, 0.0, 25.4, 25.4), 0.0, 5));
        app.editor.selection.set([r]);
        let out = run(&mut app, "image.convert_bitmap", json!({ "dpi": 50 })).unwrap();
        let new = ShapeId(out["created"].as_u64().unwrap());
        assert!(app.editor.doc.get(new).unwrap().is_image());
        assert!(app.editor.doc.get(r).is_none());
        let mask = app.editor.doc.add(Shape::ellipse(KRect::new(40.0, 45.0, 50.0, 55.0), 1));
        app.editor.selection.set([id, mask]);
        run(&mut app, "tools.mask_image", Value::Null).unwrap();
        assert!(matches!(&app.editor.doc.get(id).unwrap().kind, ShapeKind::Image(i) if i.backup.is_some()));
    }

    #[test]
    fn replace_and_refresh_by_command() {
        let (mut app, id) = app_with_image();
        let dir = temp("replace");
        let file = dir.join("new.png");
        std::fs::write(&file, png(10, 10, |_, _| 0)).unwrap();
        run(&mut app, "image.replace", json!({ "path": file.display().to_string(), "fit": true })).unwrap();
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert!((i.width - 10.0).abs() < 1e-6);
        assert_eq!(i.source.as_deref(), Some(file.display().to_string().as_str()));
        std::fs::write(&file, png(10, 10, |_, _| 128)).unwrap();
        let before = i.image.clone();
        run(&mut app, "edit.image_refresh", Value::Null).unwrap();
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_ne!(i.image, before);
        assert!(run(&mut app, "image.replace", json!({ "path": dir.join("none.png").display().to_string() })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presets_save_load_export_import_by_command() {
        let (mut app, id) = app_with_image();
        let dir = temp("presets");
        app.settings_dir = Some(dir.join("settings"));
        run(&mut app, "image.adjust", json!({ "brightness": 30, "contrast": 10, "image_mode": "stucki", "dpi": 300 })).unwrap();
        run(&mut app, "image.preset_save", json!({ "name": "Wood portrait" })).unwrap();
        assert!(dir.join("settings").join(PRESET_FILE).is_file(), "saved in the settings folder");
        run(&mut app, "image.adjust", json!({ "brightness": 0, "contrast": 0, "image_mode": "ordered", "dpi": 100 })).unwrap();
        run(&mut app, "image.preset_load", json!({ "name": "Wood portrait" })).unwrap();
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!((i.adjust.brightness, i.adjust.contrast), (30.0, 10.0));
        assert_eq!(app.editor.doc.layer(3).main().image_mode, LayerMode::Stucki);
        assert!((app.editor.doc.layer(3).main().dpi() - 300.0).abs() < 1e-6);
        let export = dir.join("export.json");
        run(&mut app, "image.preset_export", json!({ "path": export.display().to_string() })).unwrap();
        let mut other = DarkBurn::new(Prefs::default());
        let r = run(&mut other, "image.preset_import", json!({ "path": export.display().to_string() })).unwrap();
        assert_eq!(r["imported"], json!(["Wood portrait"]));
        assert_eq!(run(&mut other, "image.presets", Value::Null).unwrap()["presets"], json!(["Wood portrait"]));
        assert!(run(&mut app, "image.preset_load", json!({ "name": "Nope" })).is_err());
        assert_eq!(run(&mut app, "image.preset_delete", json!({ "name": "Wood portrait" })).unwrap()["deleted"], true);
        assert_eq!(run(&mut app, "image.presets", Value::Null).unwrap()["presets"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_offsets_by_command_and_file() {
        let (mut app, _) = app_with_image();
        let dir = temp("offsets");
        app.settings_dir = Some(dir.join("settings"));
        let prev = scan_offsets::current();
        scan_offsets::set(Default::default());
        run(&mut app, "image.scan_offsets", json!({ "speed": 300, "offset": 0.12 })).unwrap();
        run(&mut app, "image.scan_offsets", json!({ "speed": 100, "offset": 0.04, "fast_skip": false })).unwrap();
        let r = run(&mut app, "image.scan_offsets", json!({})).unwrap();
        assert_eq!(r["offsets"], json!([[100.0, 0.04], [300.0, 0.12]]));
        assert_eq!(r["fast_skip"], false);
        assert!(app.images.offsets_open, "no arguments opens the window");
        assert!(dir.join("settings").join(scan_offsets::FILE).is_file());
        let f = dir.join(format!("t.{}", scan_offsets::EXTENSION));
        run(&mut app, "image.scan_offsets_export", json!({ "path": f.display().to_string() })).unwrap();
        scan_offsets::set(Default::default());
        let r = run(&mut app, "image.scan_offsets_import", json!({ "path": f.display().to_string() })).unwrap();
        assert_eq!(r["rows"], 2);
        assert!(run(&mut app, "image.scan_offsets_import", json!({ "path": dir.join("nothing").display().to_string() })).is_err());
        let loaded = scan_offsets::ScanTuning::load_from(&dir.join("settings"));
        assert_eq!(loaded.offsets.entries.len(), 2);
        scan_offsets::set(prev);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trace_capture_hook_by_command() {
        let (mut app, _) = app_with_image();
        let dir = temp("capture");
        let f = dir.join("cap.png");
        std::fs::write(&f, png(40, 40, |x, y| if (10..30).contains(&x) && (10..30).contains(&y) { 0 } else { 255 })).unwrap();
        let r =
            run(&mut app, "image.trace_capture", json!({ "path": f.display().to_string(), "x": 100, "y": 100, "width": 40, "height": 40 })).unwrap();
        let id = ShapeId(r["created"][0].as_u64().unwrap());
        let bb = app.editor.doc.get(id).unwrap().bbox();
        assert!((bb.x0 - 110.0).abs() < 1.5 && (bb.x1 - 130.0).abs() < 1.5, "{bb:?}");
        assert!(!crate::commands::hardware("image.trace_capture"), "tracing a capture touches no machine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adjust_window_applies_live_commits_once_and_cancels_cleanly() {
        let (mut app, id) = app_with_image();
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.args.brightness = Some(25.0);
        d.args.image_mode = Some("newsprint".into());
        d.sync(&mut app);
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!(i.adjust.brightness, 25.0, "live on the canvas");
        assert_eq!(app.editor.doc.layer(3).main().image_mode, LayerMode::Newsprint);
        assert_eq!(app.editor.history.len(), 0, "no undo step while editing");
        d.restore(&mut app);
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!(i.adjust.brightness, 0.0, "cancel puts it back");
        assert_eq!(app.editor.doc.layer(3).main().image_mode, LayerMode::Dither);
        d.commit(&mut app);
        assert_eq!(app.editor.history.len(), 1, "OK is one undo step");
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!(i.adjust.brightness, 25.0);
        app.editor.undo();
        assert_eq!(app.editor.doc.layer(3).main().image_mode, LayerMode::Dither);
    }

    /// Opening Adjust Image and leaving it with Cancel (or OK with nothing changed) is not a change
    /// to the project: it used to count as modified from then on.
    #[test]
    fn cancelling_adjust_image_leaves_a_saved_project_saved() {
        let (mut app, id) = app_with_image();
        app.editor.mark_saved();
        // Looked at and cancelled.
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.sync(&mut app);
        d.restore(&mut app);
        assert!(!app.editor.is_modified(), "Cancel with nothing edited changed the project");
        // Edited, then cancelled: the edit shows as a change while the window is open.
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.args.brightness = Some(25.0);
        d.sync(&mut app);
        assert!(app.editor.is_modified());
        d.sync(&mut app);
        d.restore(&mut app);
        let ShapeKind::Image(i) = &app.editor.doc.get(id).unwrap().kind else { panic!() };
        assert_eq!(i.adjust.brightness, 0.0);
        assert!(!app.editor.is_modified(), "Cancel after an edit left the project changed");
        // OK with nothing changed: no undo step and no change.
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.sync(&mut app);
        d.commit(&mut app);
        assert_eq!(app.editor.history.len(), 0);
        assert!(!app.editor.is_modified(), "OK with nothing edited changed the project");
        // Saved while a live edit was in the document: after Cancel the file and the project differ.
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.args.brightness = Some(40.0);
        d.sync(&mut app);
        app.editor.mark_saved();
        d.sync(&mut app);
        d.restore(&mut app);
        assert!(app.editor.is_modified(), "the saved file holds the edit that Cancel took back");
        // A project with unsaved changes keeps them through a Cancel.
        app.editor.touch();
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.args.brightness = Some(10.0);
        d.sync(&mut app);
        d.restore(&mut app);
        assert!(app.editor.is_modified());
    }

    #[test]
    fn adjust_window_warns_about_memory_and_loads_presets() {
        let (mut app, id) = app_with_image();
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.sync(&mut app);
        assert!(d.warning.is_none());
        d.args.dpi = Some(25_400.0);
        d.sync(&mut app);
        assert!(d.warning.as_deref().unwrap().contains("Lower the DPI") || d.warning.as_deref().unwrap().contains("Lower"), "{:?}", d.warning);
        let p = AdjustArgs { gamma: Some(2.0), dpi: Some(200.0), image_mode: Some("jarvis".into()), ..Default::default() }.to_preset("x");
        d.apply_preset(&p);
        assert_eq!(d.args.gamma, Some(2.0));
        assert_eq!(d.args.image_mode.as_deref(), Some("jarvis"));
    }

    #[test]
    fn adjust_preview_shows_original_and_burn() {
        let (app, id) = app_with_image();
        let ctx = Context::default();
        let mut d = AdjustDialog::new(&app, vec![id]).unwrap();
        d.refresh_preview(&ctx, &app);
        let (_, before, after, cropped) = d.preview.as_ref().unwrap();
        assert!(!cropped);
        assert_eq!(before.size(), after.size());
        assert_eq!(after.size()[0], 80, "20 mm at the layer's 0.25 mm");
    }

    #[test]
    fn trace_window_previews_paths_and_clips() {
        let (app, id) = app_with_image();
        let ctx = Context::default();
        let mut d = TraceDialog::new(&app, vec![id]).unwrap();
        d.refresh(&ctx, &app);
        assert!(!d.paths.is_empty());
        let all = d.paths.len();
        let key = d.key();
        d.args.fade = 0.9;
        d.args.show_points = true;
        assert_eq!(d.key(), key, "display options do not redo the trace");
        d.clip_on = true;
        d.clip = [60.0, 0.0, 100.0, 100.0];
        assert_ne!(d.key(), key);
        d.refresh(&ctx, &app);
        assert!(d.paths.is_empty(), "the right 40 % of the picture is white: {all} before");
        assert_eq!(d.effective().clip, Some([0.6, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn convert_window_reports_size_and_limits() {
        let mut app = DarkBurn::new(Prefs::default());
        let r = app.editor.doc.add(Shape::rect(KRect::new(0.0, 0.0, 25.4, 12.7), 0.0, 0));
        let open = app.editor.doc.add(Shape::path(darkburn_core::kurbo::BezPath::from_svg("M0 0 L10 10").unwrap(), 0));
        let mut d = ConvertDialog::new(&app, vec![r]).unwrap();
        d.args.dpi = 100.0;
        d.update(&app);
        assert_eq!(d.pixels, Some((100, 50)));
        assert!(!d.outline && d.problem().is_none());
        assert_eq!(d.effective().fill, Some(true));
        assert!(ConvertDialog::new(&app, vec![open]).unwrap().outline, "open shapes default to outlines");
        d.args.dpi = 4800.0;
        d.update(&app);
        assert!(d.problem().is_none(), "a small shape is fine even at 4800");
        assert!(ConvertDialog::new(&app, vec![]).is_err());
    }

    #[test]
    fn menu_commands_open_windows() {
        let (mut app, _) = app_with_image();
        run(&mut app, "tools.adjust_image", Value::Null).unwrap();
        assert!(app.images.adjust.is_some());
        run(&mut app, "tools.trace_image", Value::Null).unwrap();
        assert!(app.images.trace.is_some());
        let shape = app.editor.doc.add(Shape::rect(KRect::new(0.0, 0.0, 5.0, 5.0), 0.0, 0));
        app.editor.selection.set([shape]);
        run(&mut app, "edit.convert_bitmap", Value::Null).unwrap();
        assert!(app.images.convert.is_some());
    }

    /// Writes a project with a photo-like image for trying the windows by hand:
    /// `DARKBURN_DEMO_OUT=path cargo test -- --ignored make_demo_project`.
    #[test]
    #[ignore = "writes a demo project for manual checks"]
    fn make_demo_project() {
        let Ok(out) = std::env::var("DARKBURN_DEMO_OUT") else { return };
        let mut app = DarkBurn::new(Prefs::default());
        image_layer(&mut app.editor.doc, 3, 0.2);
        let bytes = png(300, 200, |x, y| {
            let (dx, dy) = (x as f32 - 150.0, y as f32 - 100.0);
            let r = (dx * dx + dy * dy).sqrt();
            let v = 255.0 - (r * 1.6).min(255.0) + ((x * 7 + y * 13) % 17) as f32;
            if r < 50.0 && r > 40.0 { 20 } else { v.clamp(0.0, 255.0) as u8 }
        });
        add_image(&mut app.editor.doc, bytes, (300, 200), (90.0, 60.0), (200.0, 200.0), 3);
        app.editor.doc.add(Shape::rect(KRect::new(60.0, 60.0, 120.0, 100.0), 4.0, 1));
        let data = darkburn_core::project::save(&app.editor.doc, "demo", None).unwrap();
        std::fs::write(out, data).unwrap();
    }
}
