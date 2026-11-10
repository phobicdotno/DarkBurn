//! Document edits on images: adjust, trace, convert to bitmap, replace, refresh, crop,
//! flatten, mask and save the processed bitmap [TOOLS-19, TOOLS-20, TOOLS-22, EDIT-12,
//! EDIT-22..EDIT-28, SHAPE-02, SHAPE-29, FILE-13, IMG-18..IMG-20]. Everything here works on an
//! [`Editor`] without a window; the dialogs in `image_ui` call these.

use std::path::Path;
use std::sync::Arc;

use darkburn_core::kurbo::{Affine, BezPath, Rect, Vec2};
use darkburn_core::layer::ImageMode as LayerMode;
use darkburn_core::{Document, Editor, ImageAdjust, ImageData, ImageShape, Mode, Shape, ShapeId, ShapeKind};
use darkburn_raster::{
    AdjustPreset, GrayF, ImageMode, PaintStyle, TraceMode, TraceSettings, apply_mask, process_image, rasterize_paths, trace_image,
};
use image::DynamicImage;
use serde::Deserialize;

use crate::images;

/// Most pixels a converted bitmap may have.
pub const CONVERT_MAX_PIXELS: u64 = 80_000_000;
/// Source images above this many pixels are shrunk before tracing.
pub const TRACE_MAX_PIXELS: u64 = 16_000_000;

// ----- Selection -----

/// The selected top-level shapes that are images.
pub fn selected_images(e: &Editor) -> Vec<ShapeId> {
    e.selected().into_iter().filter(|id| e.doc.get(*id).is_some_and(Shape::is_image)).collect()
}

fn image_shape(doc: &Document, id: ShapeId) -> Result<(&Shape, &ImageShape), String> {
    let s = doc.get(id).ok_or("that shape no longer exists")?;
    match &s.kind {
        ShapeKind::Image(i) => Ok((s, i)),
        _ => Err("that shape is not an image".into()),
    }
}

fn image_mut(doc: &mut Document, id: ShapeId) -> Option<&mut ImageShape> {
    match &mut doc.get_mut(id)?.kind {
        ShapeKind::Image(i) => Some(i),
        _ => None,
    }
}

/// Runs `f` as one undo step; an `Err` from `f` leaves the document as it was.
fn try_edit<T>(e: &mut Editor, label: &str, f: impl FnOnce(&mut Document, &mut darkburn_core::Selection) -> Result<T, String>) -> Result<T, String> {
    let mut out: Option<Result<T, String>> = None;
    e.edit(label, |d, s| {
        let r = f(d, s);
        let ok = r.is_ok();
        out = Some(r);
        ok
    });
    out.unwrap_or_else(|| Err("nothing was done".into()))
}

// ----- Adjust Image -----

/// Values for Adjust Image; fields left out stay as they are. The mode, DPI and negative are
/// the image layer's, so they change the layers of the images.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AdjustArgs {
    pub brightness: Option<f64>,
    pub contrast: Option<f64>,
    pub gamma: Option<f64>,
    pub enhance_amount: Option<f64>,
    pub enhance_radius: Option<f64>,
    pub enhance_denoise: Option<f64>,
    /// threshold, ordered, atkinson, dither, stucki, jarvis, newsprint, halftone, sketch, grayscale
    pub image_mode: Option<String>,
    pub dpi: Option<f64>,
    pub negative: Option<bool>,
    pub pass_through: Option<bool>,
    pub halftone_cells: Option<f64>,
    pub halftone_angle: Option<f64>,
    pub threshold_ramp: Option<f64>,
}

pub const MODE_NAMES: [(&str, LayerMode); 10] = [
    ("threshold", LayerMode::Threshold),
    ("ordered", LayerMode::Ordered),
    ("atkinson", LayerMode::Atkinson),
    ("dither", LayerMode::Dither),
    ("stucki", LayerMode::Stucki),
    ("jarvis", LayerMode::Jarvis),
    ("newsprint", LayerMode::Newsprint),
    ("halftone", LayerMode::Halftone),
    ("sketch", LayerMode::Sketch),
    ("grayscale", LayerMode::Grayscale),
];

pub fn mode_by_name(name: &str) -> Result<LayerMode, String> {
    let n = name.trim().to_ascii_lowercase();
    MODE_NAMES.iter().find(|(k, _)| *k == n).map(|(_, m)| *m).ok_or_else(|| format!("unknown image mode {name}"))
}

pub fn mode_name(m: LayerMode) -> &'static str {
    MODE_NAMES.iter().find(|(_, x)| *x == m).map_or("dither", |(k, _)| k)
}

impl AdjustArgs {
    /// The arguments that set `adjust` and the image fields of `c`.
    pub fn from_values(adjust: &ImageAdjust, c: &darkburn_core::CutSetting) -> AdjustArgs {
        AdjustArgs {
            brightness: Some(adjust.brightness),
            contrast: Some(adjust.contrast),
            gamma: Some(adjust.gamma),
            enhance_amount: Some(adjust.enhance_amount),
            enhance_radius: Some(adjust.enhance_radius),
            enhance_denoise: Some(adjust.enhance_denoise),
            image_mode: Some(mode_name(c.image_mode).into()),
            dpi: Some(c.dpi()),
            negative: Some(c.negative),
            pass_through: Some(c.pass_through),
            halftone_cells: Some(c.halftone_cells),
            halftone_angle: Some(c.halftone_angle),
            threshold_ramp: Some(c.threshold_ramp),
        }
    }

    /// From a saved preset.
    pub fn from_preset(p: &AdjustPreset) -> AdjustArgs {
        let a = &p.adjustments;
        let mut out = AdjustArgs {
            brightness: Some(f64::from(a.brightness)),
            contrast: Some(f64::from(a.contrast)),
            gamma: Some(f64::from(a.gamma)),
            enhance_amount: Some(f64::from(a.enhance_amount) * 100.0),
            enhance_radius: Some(f64::from(a.enhance_radius)),
            enhance_denoise: Some(f64::from(a.enhance_denoise) * 100.0),
            dpi: Some(p.dpi),
            negative: Some(a.negative),
            pass_through: Some(p.pass_through),
            ..Default::default()
        };
        let name = match &p.mode {
            ImageMode::Threshold { ramp_length_mm, .. } => {
                out.threshold_ramp = Some(*ramp_length_mm);
                LayerMode::Threshold
            }
            ImageMode::Ordered => LayerMode::Ordered,
            ImageMode::Atkinson => LayerMode::Atkinson,
            ImageMode::FloydSteinberg => LayerMode::Dither,
            ImageMode::Jarvis => LayerMode::Jarvis,
            ImageMode::Stucki => LayerMode::Stucki,
            ImageMode::Newsprint => LayerMode::Newsprint,
            ImageMode::Halftone { cells_per_inch, angle_deg } => {
                out.halftone_cells = Some(*cells_per_inch);
                out.halftone_angle = Some(*angle_deg);
                LayerMode::Halftone
            }
            ImageMode::Sketch { .. } => LayerMode::Sketch,
            ImageMode::Grayscale => LayerMode::Grayscale,
            ImageMode::PassThrough => {
                out.pass_through = Some(true);
                LayerMode::Dither
            }
        };
        if !matches!(p.mode, ImageMode::PassThrough) {
            out.image_mode = Some(mode_name(name).into());
        }
        out
    }

    /// As a preset called `name`.
    pub fn to_preset(&self, name: &str) -> AdjustPreset {
        let d = ImageAdjust::default();
        let mut c = darkburn_core::CutSetting::default();
        if let Some(m) = self.image_mode.as_deref().and_then(|m| mode_by_name(m).ok()) {
            c.image_mode = m;
        }
        c.halftone_cells = self.halftone_cells.unwrap_or(c.halftone_cells);
        c.halftone_angle = self.halftone_angle.unwrap_or(c.halftone_angle);
        c.threshold_ramp = self.threshold_ramp.unwrap_or(0.0);
        c.pass_through = self.pass_through.unwrap_or(false);
        let adjust = ImageAdjust {
            brightness: self.brightness.unwrap_or(d.brightness),
            contrast: self.contrast.unwrap_or(d.contrast),
            gamma: self.gamma.unwrap_or(d.gamma),
            enhance_amount: self.enhance_amount.unwrap_or(d.enhance_amount),
            enhance_radius: self.enhance_radius.unwrap_or(d.enhance_radius),
            enhance_denoise: self.enhance_denoise.unwrap_or(d.enhance_denoise),
        };
        let mut adjustments = images::adjustments(&adjust);
        adjustments.negative = self.negative.unwrap_or(false);
        AdjustPreset {
            name: name.to_string(),
            adjustments,
            mode: images::engine_mode(&c),
            dpi: self.dpi.unwrap_or(254.0),
            pass_through: c.pass_through,
        }
    }
}

fn finite(v: f64, lo: f64, hi: f64, what: &str) -> Result<f64, String> {
    if v.is_finite() { Ok(v.clamp(lo, hi)) } else { Err(format!("{what} must be a number")) }
}

/// Applies `a` to the images `ids` and their layers; true when something changed.
pub fn adjust(e: &mut Editor, ids: &[ShapeId], a: &AdjustArgs) -> Result<bool, String> {
    let mut failed = None;
    let changed = e.edit("Adjust image", |d, _| match adjust_doc(d, ids, a) {
        Ok(changed) => changed,
        Err(err) => {
            failed = Some(err);
            false
        }
    });
    failed.map_or(Ok(changed), Err)
}

/// [`adjust`] straight on the document, with no undo step (the dialog's live preview); true
/// when anything changed.
pub fn adjust_doc(d: &mut Document, ids: &[ShapeId], a: &AdjustArgs) -> Result<bool, String> {
    if ids.is_empty() {
        return Err("select an image first".into());
    }
    let mode = a.image_mode.as_deref().map(mode_by_name).transpose()?;
    let v = |x: Option<f64>, lo, hi, what| x.map(|x| finite(x, lo, hi, what)).transpose();
    let (brightness, contrast, gamma) =
        (v(a.brightness, -100.0, 100.0, "brightness")?, v(a.contrast, -100.0, 100.0, "contrast")?, v(a.gamma, 0.1, 10.0, "gamma")?);
    let (amount, radius, denoise) = (
        v(a.enhance_amount, 0.0, 1000.0, "enhance amount")?,
        v(a.enhance_radius, 0.0, 100.0, "enhance radius")?,
        v(a.enhance_denoise, 0.0, 100.0, "enhance denoise")?,
    );
    let dpi = v(a.dpi, 1.0, 25_400.0, "DPI")?;
    let (cells, angle, ramp) = (
        v(a.halftone_cells, 1.0, 1000.0, "halftone cells")?,
        v(a.halftone_angle, -360.0, 360.0, "halftone angle")?,
        v(a.threshold_ramp, 0.0, 100.0, "ramp length")?,
    );
    let snapshot = |d: &Document| -> (Vec<Option<ImageAdjust>>, Vec<darkburn_core::Layer>) {
        (
            ids.iter().map(|id| d.get(*id).and_then(|s| if let ShapeKind::Image(i) = &s.kind { Some(i.adjust) } else { None })).collect(),
            d.layers.clone(),
        )
    };
    let before = snapshot(d);
    {
        let mut layers = Vec::new();
        for id in ids {
            let Some(img) = image_mut(d, *id) else { continue };
            let adj = &mut img.adjust;
            adj.brightness = brightness.unwrap_or(adj.brightness);
            adj.contrast = contrast.unwrap_or(adj.contrast);
            adj.gamma = gamma.unwrap_or(adj.gamma);
            adj.enhance_amount = amount.unwrap_or(adj.enhance_amount);
            adj.enhance_radius = radius.unwrap_or(adj.enhance_radius);
            adj.enhance_denoise = denoise.unwrap_or(adj.enhance_denoise);
            if let Some(s) = d.get(*id) {
                layers.push(s.layer);
            }
        }
        layers.sort_unstable();
        layers.dedup();
        for l in layers {
            let c = d.layer_mut(l).main_mut();
            if let Some(m) = mode {
                c.image_mode = m;
            }
            if let Some(x) = dpi {
                c.set_dpi(x);
            }
            c.negative = a.negative.unwrap_or(c.negative);
            c.pass_through = a.pass_through.unwrap_or(c.pass_through);
            c.halftone_cells = cells.unwrap_or(c.halftone_cells);
            c.halftone_angle = angle.unwrap_or(c.halftone_angle);
            c.threshold_ramp = ramp.unwrap_or(c.threshold_ramp);
            c.sanitise();
        }
    }
    Ok(snapshot(d) != before)
}

// ----- Trace Image -----

/// Settings for Trace Image. `clip` is a rectangle of the image as fractions `[x0, y0, x1, y1]`
/// from the top-left corner; `fade` and `show_points` only change the dialog's preview.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct TraceArgs {
    pub threshold: u8,
    pub lower_cutoff: u8,
    pub ignore_smaller: u32,
    pub smoothness: f32,
    pub optimize: f32,
    pub clip: Option<[f64; 4]>,
    pub sketch: bool,
    pub sketch_radius: f32,
    pub sketch_sensitivity: f32,
    pub delete_image: bool,
    pub fade: f32,
    pub show_points: bool,
    /// Layer for the traced paths; the current layer (or the first line layer) when left out.
    pub layer: Option<usize>,
}

impl Default for TraceArgs {
    fn default() -> Self {
        TraceArgs {
            threshold: 128,
            lower_cutoff: 0,
            ignore_smaller: 2,
            smoothness: 1.0,
            optimize: 0.5,
            clip: None,
            sketch: false,
            sketch_radius: 2.0,
            sketch_sensitivity: 0.5,
            delete_image: false,
            fade: 0.5,
            show_points: false,
            layer: None,
        }
    }
}

impl TraceArgs {
    fn settings(&self, w: u32, h: u32) -> TraceSettings {
        let clip = self.clip.map(|[x0, y0, x1, y1]| {
            let (fx, fy) = (f64::from(w), f64::from(h));
            Rect::new(x0.min(x1) * fx, y0.min(y1) * fy, x0.max(x1) * fx, y0.max(y1) * fy)
        });
        TraceSettings {
            threshold: self.threshold,
            lower_cutoff: self.lower_cutoff,
            ignore_smaller_px: self.ignore_smaller,
            smoothness: self.smoothness.clamp(0.0, 2.0),
            optimize: self.optimize.clamp(0.05, 20.0),
            clip,
            mode: if self.sketch {
                TraceMode::Sketch { radius_px: self.sketch_radius.clamp(0.5, 50.0), sensitivity: self.sketch_sensitivity.clamp(0.0, 1.0) }
            } else {
                TraceMode::Outline
            },
        }
    }
}

/// Number of nodes in `paths`.
pub fn node_count(paths: &[BezPath]) -> usize {
    paths.iter().map(|p| p.elements().len()).sum()
}

/// Traces grey pixels placed by `to_mm`; very large ones are shrunk first.
pub fn trace_gray(gray: &GrayF, to_mm: Affine, args: &TraceArgs) -> Vec<BezPath> {
    trace_gray_limited(gray, to_mm, args, TRACE_MAX_PIXELS)
}

/// [`trace_gray`] shrinking images above `max_px` pixels first (the dialog's preview uses a
/// small limit so it stays quick).
pub fn trace_gray_limited(gray: &GrayF, to_mm: Affine, args: &TraceArgs, max_px: u64) -> Vec<BezPath> {
    let px = u64::from(gray.width) * u64::from(gray.height);
    if px > max_px {
        let k = (max_px as f64 / px as f64).sqrt();
        let (nw, nh) = (((f64::from(gray.width) * k) as u32).max(1), ((f64::from(gray.height) * k) as u32).max(1));
        let small = GrayF::from_luma8(&image::imageops::resize(&gray.to_luma8(), nw, nh, image::imageops::FilterType::Triangle));
        let back = to_mm * Affine::scale_non_uniform(f64::from(gray.width) / f64::from(nw), f64::from(gray.height) / f64::from(nh));
        return trace_image(&small, back, &args.settings(nw, nh));
    }
    trace_image(gray, to_mm, &args.settings(gray.width, gray.height))
}

/// The paths tracing image `id` would give, in document millimetres (the image's adjustments
/// and crop count). Used by the dialog's preview as well.
pub fn trace_paths(doc: &Document, id: ShapeId, args: &TraceArgs) -> Result<Vec<BezPath>, String> {
    trace_paths_limited(doc, id, args, TRACE_MAX_PIXELS)
}

/// [`trace_paths`] on at most `max_px` pixels.
pub fn trace_paths_limited(doc: &Document, id: ShapeId, args: &TraceArgs, max_px: u64) -> Result<Vec<BezPath>, String> {
    let (leaf, img) = image_shape(doc, id)?;
    let gray = images::source_gray(doc, img).map_err(|e| e.to_string())?;
    let mut g = (*gray).clone();
    images::adjustments(&img.adjust).apply(&mut g);
    Ok(trace_gray_limited(&g, images::pixel_to_doc(leaf, img, (g.width, g.height)), args, max_px))
}

/// The layer traced paths go to: the one asked for, else the current layer unless it is an
/// image layer, else the first layer that is not one.
pub fn trace_layer(doc: &Document, wanted: Option<usize>, current: usize) -> usize {
    if let Some(l) = wanted {
        return l.min(darkburn_core::LAYER_COUNT - 1);
    }
    let usable = |l: usize| doc.layer(l).main().mode != Mode::Image && !darkburn_core::layer::is_tool_layer(l);
    if usable(current) { current } else { (0..darkburn_core::LAYER_COUNT).find(|l| usable(*l)).unwrap_or(0) }
}

/// What a trace made.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceReport {
    pub shapes: Vec<ShapeId>,
    pub nodes: usize,
}

fn add_traced(d: &mut Document, paths: Vec<BezPath>, layer: usize) -> Option<ShapeId> {
    let mut all = BezPath::new();
    for p in paths {
        all.extend(p);
    }
    if all.elements().is_empty() {
        return None;
    }
    Some(d.add(Shape::path(all, layer)))
}

/// Traces the images `ids` into path shapes (one per image), selecting them.
pub fn trace(e: &mut Editor, ids: &[ShapeId], args: &TraceArgs) -> Result<TraceReport, String> {
    if ids.is_empty() {
        return Err("select an image first".into());
    }
    let layer = trace_layer(&e.doc, args.layer, e.current_layer);
    let mut results = Vec::new();
    for id in ids {
        results.push((*id, trace_paths(&e.doc, *id, args)?));
    }
    if results.iter().all(|(_, p)| p.is_empty()) {
        return Err("nothing was found to trace: try a higher threshold".into());
    }
    let nodes = results.iter().map(|(_, p)| node_count(p)).sum();
    try_edit(e, "Trace image", |d, sel| {
        let mut shapes = Vec::new();
        for (id, paths) in results {
            if let Some(new) = add_traced(d, paths, layer) {
                shapes.push(new);
            }
            if args.delete_image {
                d.remove(&[id]);
            }
        }
        if args.delete_image {
            d.prune_images();
        }
        sel.set(shapes.iter().copied());
        Ok(TraceReport { shapes, nodes })
    })
}

/// Traces a camera capture: `capture` placed by `pixel_to_mm` (document millimetres, Y up) into
/// a path shape. This is the hook the camera feature calls with a frame it has already
/// corrected; it touches no hardware itself [IMG-20].
pub fn trace_capture(e: &mut Editor, capture: &DynamicImage, pixel_to_mm: Affine, args: &TraceArgs) -> Result<TraceReport, String> {
    let gray = GrayF::from_dynamic(capture);
    let paths = trace_gray(&gray, pixel_to_mm, args);
    if paths.is_empty() {
        return Err("nothing was found to trace in the capture".into());
    }
    let layer = trace_layer(&e.doc, args.layer, e.current_layer);
    let nodes = node_count(&paths);
    try_edit(e, "Trace capture", |d, sel| {
        let shapes: Vec<ShapeId> = add_traced(d, paths, layer).into_iter().collect();
        sel.set(shapes.iter().copied());
        Ok(TraceReport { shapes, nodes })
    })
}

// ----- Convert to bitmap -----

/// Settings for Convert to Bitmap.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct ConvertArgs {
    pub dpi: f64,
    /// Fill the shapes (default when they are all closed) or draw their outlines.
    pub fill: Option<bool>,
    /// Pen width for outlines, mm.
    pub stroke_mm: f64,
    pub layer: Option<usize>,
    pub keep_original: bool,
}

impl Default for ConvertArgs {
    fn default() -> Self {
        ConvertArgs { dpi: 254.0, fill: None, stroke_mm: 0.2, layer: None, keep_original: false }
    }
}

/// PNG bytes of black artwork `gray` (white = nothing) with the artwork opaque and the rest
/// transparent.
pub fn artwork_png(gray: &GrayF) -> Result<Vec<u8>, String> {
    let rgba: Vec<u8> = gray.data.iter().flat_map(|&v| [0, 0, 0, ((1.0 - v.clamp(0.0, 1.0)) * 255.0).round() as u8]).collect();
    crate::import::rgba_to_png(gray.width, gray.height, rgba).ok_or_else(|| "the bitmap could not be encoded".to_string())
}

/// Replaces the non-image shapes `ids` with one bitmap of them at `args.dpi`.
pub fn convert_to_bitmap(e: &mut Editor, ids: &[ShapeId], args: &ConvertArgs) -> Result<ShapeId, String> {
    let dpi = finite(args.dpi, 5.0, 4800.0, "DPI")?;
    let mut paths = Vec::new();
    let mut closed = true;
    let mut layer = None;
    let mut used = Vec::new();
    // Images inside the groups: they are not drawn into the bitmap, so they stay.
    let mut images_kept = Vec::new();
    for id in ids {
        let Some(top) = e.doc.get(*id) else { continue };
        for leaf in top.leaves() {
            if leaf.is_image() {
                images_kept.push(leaf);
                continue;
            }
            closed &= leaf.is_closed();
            layer.get_or_insert(leaf.layer);
            paths.push(leaf.outline());
        }
        used.push(*id);
    }
    if paths.is_empty() {
        return Err("select shapes to convert (images are bitmaps already)".into());
    }
    let fill = args.fill.unwrap_or(closed);
    let style =
        if fill { PaintStyle::Fill { even_odd: false } } else { PaintStyle::Stroke { width_mm: finite(args.stroke_mm, 0.01, 10.0, "pen width")? } };
    let raster = rasterize_paths(&paths, style, dpi, None, CONVERT_MAX_PIXELS).map_err(|e| e.to_string())?;
    let png = artwork_png(&raster.image)?;
    let (w, h) = (raster.image.width, raster.image.height);
    let px_mm = raster.image_to_mm.as_coeffs()[0];
    let (wmm, hmm) = (f64::from(w) * px_mm, f64::from(h) * px_mm);
    let bottom_left = raster.image_to_mm * darkburn_core::kurbo::Point::new(0.0, f64::from(h));
    let centre = Vec2::new(bottom_left.x + wmm / 2.0, bottom_left.y + hmm / 2.0);
    let layer = args.layer.unwrap_or_else(|| layer.unwrap_or(e.current_layer));
    try_edit(e, "Convert to bitmap", |d, sel| {
        let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(png), px_width: w, px_height: h });
        let shape =
            Shape::new(ShapeKind::Image(ImageShape { image: key, width: wmm, height: hmm, ..Default::default() }), Affine::translate(centre), layer);
        if !args.keep_original {
            d.remove(&used);
            for image in images_kept {
                d.add(image);
            }
        }
        let new = d.add(shape);
        d.prune_images();
        sel.set([new]);
        Ok(new)
    })
}

// ----- Replace and refresh -----

fn read_bitmap(bytes: &[u8]) -> Result<(ImageData, f64, f64), String> {
    let opts = crate::import::Options { page: darkburn_core::kurbo::Size::new(1e9, 1e9), ..Default::default() };
    let imp = crate::import::bitmap(bytes, &opts, None)?;
    let shape = imp.shapes.first().ok_or("the image is empty")?;
    let ShapeKind::Image(img) = &shape.kind else { return Err("the image could not be read".into()) };
    let data = imp.images.get(&img.image).ok_or("the image could not be read")?;
    Ok(((**data).clone(), img.width, img.height))
}

/// Puts the picture in `bytes` (a file read from `source`) into image `id`. With `fit` it is
/// scaled to fit the old picture's box; else it keeps its own size. The centre stays.
pub fn replace_image(e: &mut Editor, id: ShapeId, bytes: &[u8], source: Option<&Path>, fit: bool) -> Result<(), String> {
    let (old_w, old_h) = {
        let (_, img) = image_shape(&e.doc, id)?;
        (img.width, img.height)
    };
    let (data, nat_w, nat_h) = read_bitmap(bytes)?;
    let k = if fit { (old_w / nat_w).min(old_h / nat_h) } else { 1.0 };
    let source = source.map(|p| p.display().to_string());
    try_edit(e, if fit { "Replace image to fit" } else { "Replace image" }, |d, _| {
        let key = d.add_image(data);
        let img = image_mut(d, id).ok_or("that shape is not an image")?;
        img.image = key;
        (img.width, img.height) = (nat_w * k, nat_h * k);
        img.crop = None;
        img.backup = None;
        img.source = source;
        d.prune_images();
        Ok(())
    })
}

/// Reloads image `id` from the file it came from, keeping its place, size, crop and adjustments.
pub fn refresh_image(e: &mut Editor, id: ShapeId) -> Result<(), String> {
    let source = image_shape(&e.doc, id)?.1.source.clone().ok_or("this image does not remember a file to reload")?;
    let bytes = std::fs::read(&source).map_err(|err| format!("could not read {source}: {err}"))?;
    let (data, ..) = read_bitmap(&bytes)?;
    let (pw, ph) = (f64::from(data.px_width), f64::from(data.px_height));
    try_edit(e, "Refresh image", |d, _| {
        let key = d.add_image(data);
        let img = image_mut(d, id).ok_or("that shape is not an image")?;
        let (old_aspect, new_aspect) = (img.width / img.height, pw / ph);
        img.image = key;
        if (old_aspect / new_aspect - 1.0).abs() > 0.01 {
            img.height = img.width / new_aspect;
        }
        d.prune_images();
        Ok(())
    })
}

// ----- Crop and mask -----

fn crop_from(doc: &Document, image: ShapeId, others: &[ShapeId]) -> Result<BezPath, String> {
    let (leaf, _) = image_shape(doc, image)?;
    if leaf.transform.determinant().abs() < 1e-12 {
        return Err("the image has no size".into());
    }
    let inverse = leaf.transform.inverse();
    let mut crop = BezPath::new();
    for id in others {
        let s = doc.get(*id).ok_or("a crop shape no longer exists")?;
        for part in s.leaves() {
            if !part.is_closed() {
                return Err("the crop shape must be closed".into());
            }
            let mut p = part.outline();
            p.apply_affine(inverse);
            crop.extend(p);
        }
    }
    if crop.elements().is_empty() {
        return Err("the crop shape is empty".into());
    }
    Ok(crop)
}

/// Splits a selection into the one image and the shapes to crop it with.
pub fn crop_selection(doc: &Document, selected: &[ShapeId]) -> Result<(ShapeId, Vec<ShapeId>), String> {
    let images: Vec<ShapeId> = selected.iter().copied().filter(|id| doc.get(*id).is_some_and(Shape::is_image)).collect();
    let others: Vec<ShapeId> = selected.iter().copied().filter(|id| doc.get(*id).is_some_and(|s| !s.is_image())).collect();
    match (images.as_slice(), others.is_empty()) {
        ([one], false) => Ok((*one, others)),
        ([], _) => Err("select an image and the closed shape to crop it with".into()),
        (_, true) => Err("also select the closed shape to crop the image with".into()),
        _ => Err("select just one image and the shapes to crop it with".into()),
    }
}

fn set_crop(d: &mut Document, image: ShapeId, others: &[ShapeId]) -> Result<(), String> {
    let crop = crop_from(d, image, others)?;
    image_mut(d, image).ok_or("that shape is not an image")?.crop = Some(crop);
    d.remove(others);
    Ok(())
}

/// Apply Crop Path to Image: the selected closed shapes become the image's crop and go away.
pub fn apply_crop_path(e: &mut Editor, selected: &[ShapeId]) -> Result<ShapeId, String> {
    let (image, others) = crop_selection(&e.doc, selected)?;
    try_edit(e, "Apply crop path to image", |d, sel| {
        set_crop(d, image, &others)?;
        sel.set([image]);
        Ok(image)
    })
}

/// Bakes the crop of image `id` into its pixels: outside it becomes transparent, the picture is
/// trimmed to the crop and the crop path is kept as the backup path.
fn flatten(d: &mut Document, id: ShapeId) -> Result<(), String> {
    let (leaf, img) = image_shape(d, id)?;
    let crop = img.crop.clone().ok_or("this image has no crop to flatten")?;
    let data = d.images.get(&img.image).ok_or("the image's pixels are missing")?;
    let rgba = images::decode(&data.bytes)?.to_rgba8();
    let (pw, ph) = (rgba.width(), rgba.height());
    let (w, h) = (img.width, img.height);
    let local = images::local_pixel_affine(img, (pw, ph));
    let masked = apply_mask(&rgba, local, std::slice::from_ref(&crop), false);
    use darkburn_core::kurbo::Shape as _;
    let bb = crop.bounding_box().intersect(Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0));
    let (sx, sy) = (f64::from(pw) / w, f64::from(ph) / h);
    let x0 = (((bb.x0 + w / 2.0) * sx).floor().max(0.0)) as u32;
    let x1 = (((bb.x1 + w / 2.0) * sx).ceil().min(f64::from(pw))) as u32;
    let y0 = (((h / 2.0 - bb.y1) * sy).floor().max(0.0)) as u32;
    let y1 = (((h / 2.0 - bb.y0) * sy).ceil().min(f64::from(ph))) as u32;
    if bb.width() <= 0.0 || bb.height() <= 0.0 || x1 <= x0 || y1 <= y0 {
        return Err("the crop lies outside the image".into());
    }
    let cropped = image::imageops::crop_imm(&masked, x0, y0, x1 - x0, y1 - y0).to_image();
    let (nw, nh) = (cropped.width(), cropped.height());
    let png = crate::import::rgba_to_png(nw, nh, cropped.into_raw()).ok_or("the cropped picture could not be encoded")?;
    let (new_w, new_h) = (f64::from(nw) / sx, f64::from(nh) / sy);
    let centre = Vec2::new(-w / 2.0 + f64::from(x0 + x1) / 2.0 / sx, h / 2.0 - f64::from(y0 + y1) / 2.0 / sy);
    let new_transform = leaf.transform * Affine::translate(centre);
    let mut backup = crop;
    backup.apply_affine(Affine::translate(-centre));
    let key = d.add_image(ImageData { format: "png".into(), bytes: Arc::from(png), px_width: nw, px_height: nh });
    let shape = d.get_mut(id).ok_or("that shape no longer exists")?;
    shape.transform = new_transform;
    if let ShapeKind::Image(i) = &mut shape.kind {
        i.image = key;
        (i.width, i.height) = (new_w, new_h);
        i.crop = None;
        i.backup = Some(backup);
    }
    d.prune_images();
    Ok(())
}

/// Flatten Image Crop for the images in `ids` that have a crop.
pub fn flatten_crop(e: &mut Editor, ids: &[ShapeId]) -> Result<usize, String> {
    let todo: Vec<ShapeId> = ids.iter().copied().filter(|id| image_shape(&e.doc, *id).is_ok_and(|(_, i)| i.crop.is_some())).collect();
    if todo.is_empty() {
        return Err("none of the selected images has a crop".into());
    }
    try_edit(e, "Flatten image crop", |d, _| {
        for id in &todo {
            flatten(d, *id)?;
        }
        Ok(todo.len())
    })
}

/// Remove Crop from the images in `ids`.
pub fn remove_crop(e: &mut Editor, ids: &[ShapeId]) -> Result<usize, String> {
    let n = clear_field(e, ids, "Remove crop from image", |i| i.crop.take().is_some())?;
    if n == 0 { Err("none of the selected images has a crop".into()) } else { Ok(n) }
}

/// Remove Backup Path from the images in `ids`.
pub fn remove_backup(e: &mut Editor, ids: &[ShapeId]) -> Result<usize, String> {
    let n = clear_field(e, ids, "Remove backup path", |i| i.backup.take().is_some())?;
    if n == 0 { Err("none of the selected images has a backup path".into()) } else { Ok(n) }
}

fn clear_field(e: &mut Editor, ids: &[ShapeId], label: &str, f: impl Fn(&mut ImageShape) -> bool) -> Result<usize, String> {
    if ids.is_empty() {
        return Err("select an image first".into());
    }
    let mut n = 0;
    e.edit(label, |d, _| {
        for id in ids {
            if let Some(i) = image_mut(d, *id) {
                n += usize::from(f(i));
            }
        }
        n > 0
    });
    Ok(n)
}

/// Apply Mask to Image: crop with the selected shapes and bake it into the pixels at once.
pub fn mask_image(e: &mut Editor, selected: &[ShapeId]) -> Result<ShapeId, String> {
    let (image, others) = crop_selection(&e.doc, selected)?;
    try_edit(e, "Apply mask to image", |d, sel| {
        set_crop(d, image, &others)?;
        flatten(d, image)?;
        sel.set([image]);
        Ok(image)
    })
}

// ----- Save processed bitmap -----

/// The picture the laser will burn for image `id` as PNG bytes: dots in black on white at the
/// layer's resolution, with the adjustments and image mode applied.
pub fn processed_png(doc: &Document, id: ShapeId) -> Result<Vec<u8>, String> {
    let (leaf, img) = image_shape(doc, id)?;
    let c = doc.layer(leaf.layer).main();
    let settings = images::image_settings(c, 0.0);
    let (w, h) = images::size_mm(leaf, img);
    let dots = images::dots_for(w, h, settings.line_interval_mm);
    if dots > images::MAX_DOTS / 4 {
        return Err(format!("{:.0} million dots is too many to save: lower the DPI", dots as f64 / 1e6));
    }
    let gray = images::source_gray(doc, img).map_err(|e| e.to_string())?;
    let to_mm = Affine::new([w / f64::from(gray.width), 0.0, 0.0, -h / f64::from(gray.height), 0.0, h]);
    let p = process_image(&gray, to_mm, &images::adjustments(&img.adjust), &settings);
    let mut out = Vec::new();
    p.map.to_preview().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Writes the processed bitmap of image `id` to `path`.
pub fn save_processed_bitmap(doc: &Document, id: ShapeId, path: &Path) -> Result<(), String> {
    let bytes = processed_png(doc, id)?;
    std::fs::write(path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::testutil::*;
    use darkburn_raster::Adjustments;

    fn editor_with_image() -> (Editor, ShapeId) {
        let mut e = Editor::new();
        image_layer(&mut e.doc, 3, 0.25);
        let id = add_image(&mut e.doc, png(40, 20, |x, _| if x < 20 { 0 } else { 255 }), (40, 20), (20.0, 10.0), (50.0, 50.0), 3);
        e.selection.set([id]);
        (e, id)
    }

    fn img_of(e: &Editor, id: ShapeId) -> ImageShape {
        match &e.doc.get(id).unwrap().kind {
            ShapeKind::Image(i) => i.clone(),
            _ => panic!(),
        }
    }

    #[test]
    fn adjust_sets_the_image_and_its_layer_in_one_undo_step() {
        let (mut e, id) = editor_with_image();
        let args = AdjustArgs {
            brightness: Some(15.0),
            gamma: Some(1.4),
            enhance_amount: Some(80.0),
            image_mode: Some("Stucki".into()),
            dpi: Some(200.0),
            negative: Some(true),
            ..Default::default()
        };
        assert!(adjust(&mut e, &[id], &args).unwrap());
        let a = img_of(&e, id).adjust;
        assert_eq!((a.brightness, a.gamma, a.enhance_amount), (15.0, 1.4, 80.0));
        let c = e.doc.layer(3).main();
        assert_eq!(c.image_mode, LayerMode::Stucki);
        assert!((c.dpi() - 200.0).abs() < 1e-6);
        assert!(c.negative);
        assert_eq!(e.history.len(), 1);
        e.undo();
        assert_eq!(img_of(&e, id).adjust, ImageAdjust::default());
        assert_eq!(e.doc.layer(3).main().image_mode, LayerMode::Dither);
    }

    #[test]
    fn adjust_refuses_bad_input() {
        let (mut e, id) = editor_with_image();
        assert!(adjust(&mut e, &[], &AdjustArgs::default()).is_err());
        assert!(adjust(&mut e, &[id], &AdjustArgs { image_mode: Some("sparkle".into()), ..Default::default() }).is_err());
        assert!(adjust(&mut e, &[id], &AdjustArgs { gamma: Some(f64::NAN), ..Default::default() }).is_err());
        assert!(!adjust(&mut e, &[id], &AdjustArgs { brightness: Some(500.0), ..Default::default() }).is_err());
        assert_eq!(img_of(&e, id).adjust.brightness, 100.0, "clamped");
    }

    #[test]
    fn presets_round_trip_through_the_arguments() {
        let (mut e, id) = editor_with_image();
        let args = AdjustArgs {
            brightness: Some(-10.0),
            contrast: Some(25.0),
            gamma: Some(0.8),
            enhance_amount: Some(120.0),
            enhance_radius: Some(2.0),
            enhance_denoise: Some(10.0),
            image_mode: Some("halftone".into()),
            halftone_cells: Some(60.0),
            halftone_angle: Some(30.0),
            dpi: Some(300.0),
            negative: Some(true),
            ..Default::default()
        };
        let preset = args.to_preset("Portrait");
        assert_eq!(preset.name, "Portrait");
        let back = AdjustArgs::from_preset(&preset);
        adjust(&mut e, &[id], &back).unwrap();
        let a = img_of(&e, id).adjust;
        assert!((a.enhance_amount - 120.0).abs() < 1e-3 && (a.enhance_denoise - 10.0).abs() < 1e-3);
        assert_eq!((a.brightness, a.contrast), (-10.0, 25.0));
        let c = e.doc.layer(3).main();
        assert_eq!((c.image_mode, c.halftone_cells, c.halftone_angle), (LayerMode::Halftone, 60.0, 30.0));
        assert!(c.negative && (c.dpi() - 300.0).abs() < 1e-6);
        let pt = AdjustArgs { pass_through: Some(true), ..Default::default() }.to_preset("pt");
        assert_eq!(pt.mode, ImageMode::PassThrough);
        assert_eq!(AdjustArgs::from_preset(&pt).pass_through, Some(true));
    }

    #[test]
    fn tracing_makes_paths_where_the_image_is_dark() {
        let (mut e, id) = editor_with_image();
        let r = trace(&mut e, &[id], &TraceArgs { layer: Some(1), ..Default::default() }).unwrap();
        assert_eq!(r.shapes.len(), 1);
        let s = e.doc.get(r.shapes[0]).unwrap();
        assert_eq!(s.layer, 1);
        use darkburn_core::kurbo::Shape as _;
        let bb = s.outline().bounding_box();
        // The left half of a 20 x 10 mm picture centred at (50, 50).
        assert!((bb.x0 - 40.0).abs() < 0.6 && (bb.x1 - 50.0).abs() < 0.6 && (bb.y0 - 45.0).abs() < 0.6 && (bb.y1 - 55.0).abs() < 0.6, "{bb:?}");
        assert!(e.doc.get(id).is_some(), "the image stays unless asked");
        assert_eq!(e.selected(), r.shapes);
    }

    #[test]
    fn trace_options_clip_delete_and_refuse() {
        let (mut e, id) = editor_with_image();
        let clipped = trace_paths(&e.doc, id, &TraceArgs { clip: Some([0.0, 0.0, 0.25, 1.0]), ..Default::default() }).unwrap();
        use darkburn_core::kurbo::Shape as _;
        let mut all = BezPath::new();
        clipped.iter().for_each(|p| all.extend(p.clone()));
        assert!(all.bounding_box().x1 < 46.0, "only the leftmost quarter: {:?}", all.bounding_box());
        // A threshold of 0 finds nothing dark enough.
        assert!(trace(&mut e, &[id], &TraceArgs { threshold: 0, ..Default::default() }).is_err());
        let before = e.doc.len();
        trace(&mut e, &[id], &TraceArgs { delete_image: true, ..Default::default() }).unwrap();
        assert_eq!(e.doc.len(), before, "the image is replaced by the paths");
        assert!(e.doc.get(id).is_none());
        assert!(e.doc.images.is_empty(), "unused pixels go too");
        e.undo();
        assert!(e.doc.get(id).is_some());
    }

    #[test]
    fn sketch_trace_gives_open_lines_and_the_trace_layer_avoids_image_layers() {
        let mut e = Editor::new();
        let id = add_image(
            &mut e.doc,
            png(40, 40, |x, y| if (18..22).contains(&x) && y > 5 && y < 35 { 0 } else { 255 }),
            (40, 40),
            (20.0, 20.0),
            (50.0, 50.0),
            3,
        );
        let paths = trace_paths(&e.doc, id, &TraceArgs { sketch: true, ..Default::default() }).unwrap();
        assert!(!paths.is_empty());
        image_layer(&mut e.doc, 0, 0.1);
        assert_eq!(trace_layer(&e.doc, None, 0), 1, "layer 0 is an image layer");
        assert_eq!(trace_layer(&e.doc, None, 5), 5);
        assert_eq!(trace_layer(&e.doc, Some(7), 5), 7);
    }

    #[test]
    fn camera_captures_trace_through_the_hook() {
        let mut e = Editor::new();
        let cap = DynamicImage::ImageLuma8(image::GrayImage::from_fn(50, 50, |x, y| {
            image::Luma([if (10..30).contains(&x) && (10..30).contains(&y) { 0 } else { 255 }])
        }));
        let r = trace_capture(&mut e, &cap, Affine::new([2.0, 0.0, 0.0, -2.0, 0.0, 100.0]), &TraceArgs::default()).unwrap();
        use darkburn_core::kurbo::Shape as _;
        let bb = e.doc.get(r.shapes[0]).unwrap().outline().bounding_box();
        assert!((bb.x0 - 20.0).abs() < 1.5 && (bb.x1 - 60.0).abs() < 1.5, "{bb:?}");
        assert!(
            trace_capture(
                &mut e,
                &DynamicImage::ImageLuma8(image::GrayImage::from_pixel(10, 10, image::Luma([255]))),
                Affine::IDENTITY,
                &TraceArgs::default()
            )
            .is_err()
        );
    }

    #[test]
    fn convert_to_bitmap_replaces_shapes_with_a_picture_of_them() {
        let mut e = Editor::new();
        let r = e.doc.add(Shape::rect(Rect::new(10.0, 10.0, 35.4, 22.7), 0.0, 4));
        let c = e.doc.add(Shape::ellipse(Rect::new(50.0, 10.0, 60.0, 20.0), 4));
        let new = convert_to_bitmap(&mut e, &[r, c], &ConvertArgs { dpi: 100.0, ..Default::default() }).unwrap();
        assert_eq!(e.doc.len(), 1);
        assert_eq!(e.selected(), vec![new]);
        let img = img_of(&e, new);
        assert!((img.width - 50.0).abs() < 0.5 && (img.height - 12.7).abs() < 0.5, "{} x {}", img.width, img.height);
        let data = &e.doc.images[&img.image];
        assert_eq!(data.px_width, (img.width / 0.254).round() as u32);
        let bb = e.doc.get(new).unwrap().bbox();
        assert!((bb.x0 - 10.0).abs() < 0.3 && (bb.y0 - 10.0).abs() < 0.3, "{bb:?}");
        assert_eq!(e.doc.get(new).unwrap().layer, 4);
        // The picture has opaque black where the rectangle was and nothing elsewhere.
        let rgba = images::decode(&data.bytes).unwrap().to_rgba8();
        assert_eq!(rgba.get_pixel(5, rgba.height() - 5)[3], 255);
        assert_eq!(rgba.get_pixel(rgba.width() - 3, 1)[3], 0);
        e.undo();
        assert_eq!(e.doc.len(), 2);
    }

    /// A group with a picture in it: the vector shapes become the bitmap, the picture is not
    /// drawn into it and must not vanish with the group.
    #[test]
    fn convert_to_bitmap_keeps_the_images_of_a_group() {
        let (mut e, image) = editor_with_image();
        let was = e.doc.get(image).unwrap().bbox();
        let r = e.doc.add(Shape::rect(Rect::new(100.0, 100.0, 120.0, 110.0), 0.0, 4));
        let group = darkburn_core::arrange::group(&mut e.doc, &[image, r]).unwrap();
        let new = convert_to_bitmap(&mut e, &[group], &ConvertArgs { dpi: 100.0, ..Default::default() }).unwrap();
        let images: Vec<Rect> = e.doc.iter().filter(|s| s.is_image() && s.id != new).map(Shape::bbox).collect();
        assert_eq!(images.len(), 1, "the picture in the group was deleted with it");
        assert!((images[0].x0 - was.x0).abs() < 1e-9 && (images[0].y1 - was.y1).abs() < 1e-9, "{:?} moved from {was:?}", images[0]);
        assert_eq!(e.doc.len(), 2, "the group's rectangle became the bitmap");
        assert!((e.doc.get(new).unwrap().bbox().width() - 20.0).abs() < 0.5, "the bitmap is of the rectangle alone");
        e.undo();
        assert_eq!(e.doc.len(), 1, "one undo step gives the group back");
    }

    #[test]
    fn convert_to_bitmap_options_and_refusals() {
        let mut e = Editor::new();
        let r = e.doc.add(Shape::rect(Rect::new(0.0, 0.0, 20.0, 20.0), 0.0, 0));
        assert!(convert_to_bitmap(&mut e, &[], &ConvertArgs::default()).is_err());
        assert!(convert_to_bitmap(&mut e, &[r], &ConvertArgs { dpi: f64::NAN, ..Default::default() }).is_err());
        let huge = e.doc.add(Shape::rect(Rect::new(0.0, 0.0, 5000.0, 5000.0), 0.0, 0));
        let err = convert_to_bitmap(&mut e, &[huge], &ConvertArgs { dpi: 1200.0, ..Default::default() }).unwrap_err();
        assert!(err.contains("larger than the limit"), "{err}");
        let kept =
            convert_to_bitmap(&mut e, &[r], &ConvertArgs { dpi: 50.0, fill: Some(false), keep_original: true, layer: Some(9), ..Default::default() })
                .unwrap();
        assert!(e.doc.get(r).is_some());
        assert_eq!(e.doc.get(kept).unwrap().layer, 9);
    }

    #[test]
    fn replace_and_refresh_swap_the_pixels_but_keep_the_place() {
        let (mut e, id) = editor_with_image();
        let new = png(10, 10, |_, _| 0);
        replace_image(&mut e, id, &new, Some(Path::new("c:/x/new.png")), true).unwrap();
        let img = img_of(&e, id);
        assert!((img.width - 10.0).abs() < 1e-6 && (img.height - 10.0).abs() < 1e-6, "fits the old 20 x 10 box: {} x {}", img.width, img.height);
        assert_eq!(img.source.as_deref(), Some("c:/x/new.png"));
        assert_eq!(e.doc.images.len(), 1, "the old picture is dropped");
        assert!((e.doc.get(id).unwrap().bbox().center().x - 50.0).abs() < 1e-6);
        replace_image(&mut e, id, &new, None, false).unwrap();
        let natural = img_of(&e, id);
        assert!((natural.width - 10.0 * 25.4 / 254.0).abs() < 1e-6, "natural size at the default DPI");
        assert!(replace_image(&mut e, id, b"not an image", None, false).is_err());
        e.undo();
        e.undo();
        assert_eq!(img_of(&e, id).source, None);
    }

    #[test]
    fn refresh_reloads_from_the_source_file() {
        let (mut e, id) = editor_with_image();
        let dir = std::env::temp_dir().join(format!("darkburn-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pic.png");
        std::fs::write(&file, png(40, 20, |_, _| 0)).unwrap();
        assert!(refresh_image(&mut e, id).is_err(), "no source yet");
        image_mut(&mut e.doc, id).unwrap().source = Some(file.display().to_string());
        let before = img_of(&e, id).image;
        refresh_image(&mut e, id).unwrap();
        let after = img_of(&e, id);
        assert_ne!(after.image, before);
        assert!((after.width - 20.0).abs() < 1e-6, "size kept when the shape is the same");
        std::fs::remove_file(&file).unwrap();
        assert!(refresh_image(&mut e, id).is_err(), "a missing file is reported");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crop_path_flatten_remove_and_backup() {
        let (mut e, id) = editor_with_image();
        let hole = e.doc.add(Shape::rect(Rect::new(40.0, 45.0, 50.0, 55.0), 0.0, 1));
        let open = e.doc.add(Shape::path(BezPath::from_svg("M0 0 L10 10").unwrap(), 1));
        assert!(apply_crop_path(&mut e, &[id]).is_err());
        assert!(apply_crop_path(&mut e, &[id, open]).is_err(), "open shapes cannot crop");
        let n = e.doc.len();
        apply_crop_path(&mut e, &[id, hole]).unwrap();
        assert_eq!(e.doc.len(), n - 1, "the crop shape is used up");
        let img = img_of(&e, id);
        let crop = img.crop.clone().unwrap();
        use darkburn_core::kurbo::Shape as _;
        let bb = crop.bounding_box();
        assert!((bb.x0 + 10.0).abs() < 1e-6 && (bb.x1).abs() < 1e-6, "in the image's own coordinates: {bb:?}");
        let outline = e.doc.get(id).unwrap().bbox();
        assert!((outline.width() - 10.0).abs() < 1e-6, "the outline is the crop now");
        flatten_crop(&mut e, &[id]).unwrap();
        let flat = img_of(&e, id);
        assert!(flat.crop.is_none() && flat.backup.is_some());
        assert!((flat.width - 10.0).abs() < 0.1 && (flat.height - 10.0).abs() < 0.6, "trimmed to the crop: {} x {}", flat.width, flat.height);
        let pos = e.doc.get(id).unwrap().bbox();
        assert!((pos.center().x - 45.0).abs() < 0.1 && (pos.center().y - 50.0).abs() < 0.6, "stays where it was: {pos:?}");
        assert_eq!(e.doc.images.len(), 1);
        assert!(flatten_crop(&mut e, &[id]).is_err());
        assert!(remove_crop(&mut e, &[id]).is_err());
        assert_eq!(remove_backup(&mut e, &[id]).unwrap(), 1);
        assert!(img_of(&e, id).backup.is_none());
        assert!(remove_backup(&mut e, &[id]).is_err());
    }

    #[test]
    fn remove_crop_restores_the_full_picture() {
        let (mut e, id) = editor_with_image();
        let hole = e.doc.add(Shape::ellipse(Rect::new(40.0, 45.0, 60.0, 55.0), 1));
        apply_crop_path(&mut e, &[id, hole]).unwrap();
        assert_eq!(remove_crop(&mut e, &[id]).unwrap(), 1);
        assert!(img_of(&e, id).crop.is_none());
        assert!((e.doc.get(id).unwrap().bbox().width() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn mask_image_bakes_the_shape_into_the_pixels() {
        let (mut e, id) = editor_with_image();
        let shape = e.doc.add(Shape::rect(Rect::new(40.0, 45.0, 50.0, 55.0), 0.0, 1));
        mask_image(&mut e, &[id, shape]).unwrap();
        let img = img_of(&e, id);
        assert!(img.crop.is_none() && img.backup.is_some());
        let rgba = images::decode(&e.doc.images[&img.image].bytes).unwrap().to_rgba8();
        assert!(rgba.pixels().all(|p| p[3] > 0 && p[0] < 20), "all that is left is the black left half");
        assert_eq!(e.history.len(), 1, "one undo step");
        e.undo();
        assert_eq!(e.doc.len(), 2);
        assert!(img_of(&e, id).crop.is_none() && img_of(&e, id).backup.is_none());
    }

    #[test]
    fn the_processed_bitmap_is_the_burn() {
        let (mut e, id) = editor_with_image();
        e.doc.layer_mut(3).main_mut().image_mode = LayerMode::Threshold;
        let bytes = processed_png(&e.doc, id).unwrap();
        let png = image::load_from_memory(&bytes).unwrap().to_luma8();
        assert_eq!(png.width(), 80, "20 mm at 0.25 mm per dot");
        assert_eq!(png.get_pixel(5, 10)[0], 0, "black half burns");
        assert_eq!(png.get_pixel(75, 10)[0], 255);
        let dir = std::env::temp_dir().join(format!("darkburn-bmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        save_processed_bitmap(&e.doc, id, &dir.join("out.png")).unwrap();
        assert!(dir.join("out.png").is_file());
        let _ = std::fs::remove_dir_all(&dir);
        // Neutral adjustments really are neutral for the engine.
        assert!(Adjustments::default().is_neutral());
    }

    #[test]
    fn selection_helpers() {
        let (mut e, id) = editor_with_image();
        let r = e.doc.add(Shape::rect(Rect::new(0.0, 0.0, 5.0, 5.0), 0.0, 0));
        e.selection.set([id, r]);
        assert_eq!(selected_images(&e), vec![id]);
        assert_eq!(crop_selection(&e.doc, &[id, r]).unwrap(), (id, vec![r]));
        assert!(crop_selection(&e.doc, &[r]).is_err());
        assert_eq!(mode_by_name(" Halftone ").unwrap(), LayerMode::Halftone);
        assert_eq!(mode_name(LayerMode::Jarvis), "jarvis");
    }
}
