//! The camera, rotary, cylinder, print-and-cut and test-tool commands, and the working state
//! behind them. Everything here runs without a window: the dialogs in `vision_ui.rs` call the
//! same functions the registered commands do. No camera is opened here; frames come from files
//! or the canned source, and a live camera is started only by the Start button in the panel.
//!
//! The setup itself lives in the document ([`VisionState`], saved with the project); the images
//! (frames, the overlay) are working state and are not saved.

use std::path::{Path, PathBuf};

use darkburn_core::kurbo::{Point, Rect, Shape as _};
use darkburn_core::{ImageShape, ShapeId, ShapeKind};
use darkburn_vision::calibrate::{CalibOptions, calibrate_from_images};
use darkburn_vision::camera::{
    self, AlignmentTarget, BedRect, CameraAlignment, CameraKind, HeadCameraCalibration, HeadMark, ImageOrientation, MosaicCapture, default_targets,
};
use darkburn_vision::capture::FrameSource;
use darkburn_vision::centre::fit_circle;
use darkburn_vision::cylinder::{CylinderAxis, CylinderKind};
use darkburn_vision::detect::{PatternKind, PatternSpec};
use darkburn_vision::feeder;
use darkburn_vision::lens::LensKind;
use darkburn_vision::printcut::{PnCEvent, PnCMode, PnCWizard};
use darkburn_vision::rotary::{self, ObjectSize, RotaryAxis, RotaryKind};
use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;
use crate::vision_state::{SplitMode, VisionState};
use crate::vision_tools;

/// Largest frame accepted, in pixels on either side.
const MAX_FRAME: u32 = 8192;

// ----- Working state ----------------------------------------------------------------------------

/// What the app remembers about cameras between runs (CAM-09), in the settings folder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionPrefs {
    /// The user allowed live capture (off until turned on in the camera panel).
    pub live_enabled: bool,
    pub last_camera_name: String,
    pub last_device_index: Option<u32>,
    /// Camera settings saved with Save Settings: the starting point for new projects.
    pub saved_camera: Option<camera::CaptureSettings>,
}

/// The camera picture, warped onto the bed.
pub struct Overlay {
    pub img: RgbaImage,
    /// The bed area the picture covers, mm.
    pub covers: BedRect,
}

/// A frame source that plays one image over and over: a file chosen by the user.
pub struct StillFrame(pub RgbaImage);

impl FrameSource for StillFrame {
    fn grab(&mut self) -> Result<RgbaImage, darkburn_vision::capture::CaptureError> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
pub struct Vision {
    /// The setup the window works on; follows the document (undo, open) via [`sync`].
    pub state: VisionState,
    synced: Option<Value>,
    loaded: bool,
    pub prefs: VisionPrefs,
    /// The latest frame as the camera delivered it.
    pub raw: Option<RgbaImage>,
    pub overlay: Option<Overlay>,
    /// Counts overlay rebuilds, so the painter knows when to upload again.
    pub overlay_gen: u64,
    /// Counts new camera pictures.
    pub frame_gen: u64,
    /// Where frames come from now; None until the user picks a file or starts a camera.
    pub source: Option<Box<dyn FrameSource>>,
    pub source_name: String,
    /// The source is a live camera the user started.
    pub live: bool,
    pub cameras: Vec<(u32, String)>,
    pub message: Option<String>,
    // Calibration wizard working data.
    pub calib_frames: Vec<GrayImage>,
    pub calib_result: Option<String>,
    // Alignment wizard.
    pub align_count: usize,
    pub align_margin: f64,
    // Head-mounted camera.
    pub marks: Vec<HeadMark>,
    pub mosaic: Vec<MosaicCapture>,
    /// The dialogs and fields of vision_ui.rs.
    pub ui: crate::vision_ui::VisionDialogs,
}

fn project_error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ----- State and the document -------------------------------------------------------------------

/// Bring the working state in line with the document (after undo, open, new). Cheap.
pub fn sync(app: &mut DarkBurn) {
    if !app.vision.loaded {
        app.vision.loaded = true;
        load_prefs(app);
        app.vision.synced = Some(Value::Null); // force the first read below
    }
    if app.vision.synced.as_ref() != app.editor.doc.vision_settings.as_ref() || app.vision.synced == Some(Value::Null) {
        app.vision.state = VisionState::from_doc(&app.editor.doc);
        if app.editor.doc.vision_settings.is_none()
            && let Some(saved) = app.vision.prefs.saved_camera.clone()
        {
            app.vision.state.camera = saved;
        }
        app.vision.synced = app.editor.doc.vision_settings.clone();
        // The picture is for the old setup; redo it when there is a frame.
        if app.vision.raw.is_some() {
            let _ = rebuild_overlay(app);
        }
    }
}

/// Write the working state to the document. `undo` makes it an undo step (setup dialogs);
/// the camera panel's sliders pass false.
pub fn commit(app: &mut DarkBurn, label: &str, undo: bool) {
    let new = if app.vision.state.is_default() { None } else { Some(app.vision.state.to_value()) };
    if app.editor.doc.vision_settings != new {
        if undo {
            let v = new.clone();
            app.editor.edit(label, move |d, _| {
                d.vision_settings = v;
                true
            });
        } else {
            app.editor.doc.vision_settings = new.clone();
            // Not an undo step, but still a change the project file does not have yet.
            app.editor.touch();
        }
    }
    app.vision.synced = new;
}

fn prefs_path(app: &DarkBurn) -> Option<PathBuf> {
    app.settings_dir.as_ref().map(|d| d.join("camera.json"))
}

fn load_prefs(app: &mut DarkBurn) {
    if let Some(p) = prefs_path(app)
        && let Ok(text) = std::fs::read_to_string(p)
        && let Ok(prefs) = serde_json::from_str::<VisionPrefs>(&text)
    {
        app.vision.prefs = prefs;
    }
}

pub fn save_prefs(app: &mut DarkBurn) -> Result<(), String> {
    let Some(p) = prefs_path(app) else { return Err("there is no settings folder".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(project_error)?;
    }
    let text = serde_json::to_string_pretty(&app.vision.prefs).map_err(project_error)?;
    std::fs::write(p, text).map_err(project_error)
}

/// The work area as a bed rectangle.
pub fn bed(app: &DarkBurn) -> BedRect {
    let page = app.prefs.page();
    BedRect::new(0.0, 0.0, f64::from(page.x), f64::from(page.y))
}

// ----- Frames and the overlay -------------------------------------------------------------------

/// Take `img` as the current camera picture and redo the overlay.
pub fn set_frame(app: &mut DarkBurn, img: RgbaImage) -> Result<(), String> {
    if img.width() == 0 || img.height() == 0 || img.width() > MAX_FRAME || img.height() > MAX_FRAME {
        return Err(format!("the picture must be between 1 and {MAX_FRAME} pixels on each side"));
    }
    app.vision.raw = Some(img);
    app.vision.frame_gen += 1;
    rebuild_overlay(app)
}

/// Load a picture file as the frame (a stand-in for a camera, and the way tests feed one).
pub fn load_frame_file(app: &mut DarkBurn, path: &Path) -> Result<(u32, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let img = crate::images::decode(&bytes)?.to_rgba8();
    let size = (img.width(), img.height());
    app.vision.source = Some(Box::new(StillFrame(img.clone())));
    app.vision.source_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    app.vision.live = false;
    set_frame(app, img)?;
    Ok(size)
}

/// The raw frame with the lens correction applied.
pub fn undistorted(app: &DarkBurn) -> Option<RgbaImage> {
    let raw = app.vision.raw.as_ref()?;
    let cam = &app.vision.state.camera;
    Some(match &cam.lens {
        Some(l) if l.width == raw.width() && l.height == raw.height() => l.undistort_image(raw, cam.focal_scale),
        _ => raw.clone(),
    })
}

fn extent(al: &CameraAlignment, bed: BedRect) -> BedRect {
    let (w, h) = (al.image_size.0 as f64, al.image_size.1 as f64);
    let corners = [Point::new(0.0, 0.0), Point::new(w, 0.0), Point::new(w, h), Point::new(0.0, h)];
    let mapped: Option<Vec<Point>> = corners.iter().map(|c| al.image_to_bed(*c)).collect();
    let Some(m) = mapped else { return bed };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &m {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    // Never more than the bed plus one bed size around it.
    let (lx, ly) = (bed.width, bed.height);
    let r = Rect::new(x0.max(bed.x - lx), y0.max(bed.y - ly), x1.min(bed.x + 2.0 * lx), y1.min(bed.y + 2.0 * ly));
    if r.width() < 1.0 || r.height() < 1.0 || !r.width().is_finite() { bed } else { BedRect::new(r.x0, r.y0, r.width(), r.height()) }
}

/// Redo the overlay from the current frame: lens correction, alignment to the bed, crop.
pub fn rebuild_overlay(app: &mut DarkBurn) -> Result<(), String> {
    let Some(und) = undistorted(app) else { return Err("there is no camera picture yet".into()) };
    let bed = bed(app);
    let cam = app.vision.state.camera.clone();
    let overlay = match (&cam.alignment, cam.kind) {
        (Some(al), CameraKind::Overhead) => {
            if al.image_size != (und.width(), und.height()) {
                return Err("the camera alignment was made at another picture size; run the alignment again".into());
            }
            let covers = if cam.crop_to_page { bed } else { extent(al, bed) };
            let ppm = (1600.0 / covers.width.max(covers.height)).clamp(0.25, 4.0);
            let mut img = al.render_bed_image(&und, covers, ppm);
            if cam.crop_to_page {
                img = camera::crop_to_page(&img, covers, ppm, bed.to_rect());
            }
            Overlay { img, covers }
        }
        (_, CameraKind::HeadMounted) if !app.vision.mosaic.is_empty() => {
            return Err("the head camera picture is made with Stitch".into());
        }
        _ => {
            // No alignment yet: stretch the picture over the bed so it can be seen.
            let (w, h) = bed.pixel_size(2.0);
            let (w, h) = (w.min(1600), h.min(1600));
            Overlay { img: image::imageops::resize(&und, w, h, image::imageops::FilterType::Triangle), covers: bed }
        }
    };
    app.vision.overlay = Some(overlay);
    app.vision.overlay_gen += 1;
    Ok(())
}

/// Update Overlay: take a new frame from the source (or redo the one we have) [CAMP-03].
pub fn update_overlay(app: &mut DarkBurn) -> Result<Value, String> {
    sync(app);
    if app.vision.live {
        // The viewfinder already holds the latest picture.
        rebuild_overlay(app)?;
    } else if let Some(src) = app.vision.source.as_mut() {
        let img = src.grab().map_err(|e| e.to_string())?;
        set_frame(app, img)?;
    } else {
        rebuild_overlay(app)?;
    }
    let o = app.vision.overlay.as_ref().ok_or("no overlay")?;
    Ok(json!({ "overlay": { "width_px": o.img.width(), "height_px": o.img.height(), "covers": o.covers } }))
}

/// The overlay as an image for tracing or saving: transparent areas become white paper.
pub fn overlay_flat(app: &DarkBurn) -> Result<(image::DynamicImage, camera::OverlayAdjust, BedRect), String> {
    let o = app.vision.overlay.as_ref().ok_or("there is no camera overlay yet: update the overlay first")?;
    let mut img = o.img.clone();
    for p in img.pixels_mut() {
        if p.0[3] < 255 {
            let a = u16::from(p.0[3]);
            for c in &mut p.0[..3] {
                *c = ((u16::from(*c) * a + 255 * (255 - a)) / 255) as u8;
            }
            p.0[3] = 255;
        }
    }
    Ok((image::DynamicImage::ImageRgba8(img), app.vision.state.camera.overlay, o.covers))
}

/// Trace the overlay straight into path shapes (the headless twin of the Trace button).
pub fn trace_overlay(app: &mut DarkBurn, args: &Value) -> Result<Value, String> {
    let (img, adjust, covers) = overlay_flat(app)?;
    let t: crate::image_ops::TraceArgs =
        serde_json::from_value(if args.is_null() { json!({}) } else { args.clone() }).map_err(|e| format!("trace settings: {e}"))?;
    let to_mm = adjust.pixel_to_canvas(covers, img.width(), img.height());
    let r = crate::image_ops::trace_capture(&mut app.editor, &img, to_mm, &t)?;
    Ok(json!({ "created": r.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "nodes": r.nodes }))
}

/// The overlay, encoded and placed, ready to go into the document.
pub struct OverlayPicture {
    png: Vec<u8>,
    size_px: (u32, u32),
    rect: Rect,
}

/// The overlay as a picture for [`add_overlay_picture`]; fails without one.
pub fn overlay_picture(app: &DarkBurn) -> Result<OverlayPicture, String> {
    let (img, adjust, covers) = overlay_flat(app)?;
    let size_px = (img.width(), img.height());
    let rect = adjust.placement(covers);
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(project_error)?;
    Ok(OverlayPicture { png, size_px, rect })
}

/// Put the picture in the document (selected), as the undo step "Camera picture".
pub fn add_overlay_picture(app: &mut DarkBurn, p: OverlayPicture) -> ShapeId {
    let OverlayPicture { png, size_px: (w, h), rect } = p;
    let layer = app.editor.current_layer;
    let mut id = ShapeId(0);
    app.editor.edit("Camera picture", |d, sel| {
        let key = d.add_image(darkburn_core::ImageData { format: "png".into(), bytes: png.into(), px_width: w, px_height: h });
        let shape = darkburn_core::Shape::new(
            ShapeKind::Image(ImageShape { image: key, width: rect.width(), height: rect.height(), source: None, ..Default::default() }),
            darkburn_core::kurbo::Affine::translate(rect.center().to_vec2()),
            layer,
        );
        id = d.add(shape);
        sel.set([id]);
        true
    });
    id
}

/// Save Background Capture [FILE-14]: the overlay (or the raw frame when there is none) as a picture file.
pub fn save_capture(app: &mut DarkBurn, path: &Path) -> Result<Value, String> {
    let img = match (&app.vision.overlay, &app.vision.raw) {
        (Some(o), _) => o.img.clone(),
        (None, Some(r)) => r.clone(),
        _ => return Err("there is no camera picture to save".into()),
    };
    img.save(path).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(json!({ "path": path.display().to_string(), "width_px": img.width(), "height_px": img.height() }))
}

// ----- Small argument helpers -------------------------------------------------------------------

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn flag(a: &Value, k: &str) -> Option<bool> {
    a.get(k).and_then(Value::as_bool)
}

fn text<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn point_of(v: &Value) -> Result<Point, String> {
    match (v.get("x").and_then(Value::as_f64), v.get("y").and_then(Value::as_f64)) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Point::new(x, y)),
        _ => Err("a point needs numbers x and y".into()),
    }
}

fn points(a: &Value, k: &str) -> Result<Vec<Point>, String> {
    a.get(k).and_then(Value::as_array).ok_or_else(|| format!("argument {k} must be a list of points"))?.iter().map(point_of).collect()
}

fn path_arg(a: &Value) -> Result<PathBuf, String> {
    text(a, "path").map(PathBuf::from).ok_or_else(|| "argument path must be text".into())
}

fn layer_arg(app: &DarkBurn, a: &Value, k: &str, default: usize) -> Result<usize, String> {
    let _ = app;
    match a.get(k) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|l| *l < darkburn_core::layer::COLOUR_LAYERS as u64)
            .map(|l| l as usize)
            .ok_or_else(|| format!("{k} must be a colour layer (0 to 29)")),
    }
}

// ----- Camera -------------------------------------------------------------------------------------

/// Everything the camera panel shows, as data.
pub fn camera_json(app: &DarkBurn) -> Value {
    let c = &app.vision.state.camera;
    json!({
        "settings": c,
        "source": app.vision.source_name,
        "live": app.vision.live,
        "has_frame": app.vision.raw.is_some(),
        "has_overlay": app.vision.overlay.is_some(),
        "calibrated": c.lens.is_some(),
        "aligned": c.alignment.is_some(),
        "live_allowed": app.vision.prefs.live_enabled,
        "live_built": live_built(),
    })
}

/// Whether this build can talk to a live camera at all.
pub const fn live_built() -> bool {
    cfg!(feature = "live-camera")
}

/// The lens correction changed (a new calibration, none any more, another focal scale): an
/// alignment made before was clicked in the picture as it was corrected then, so it no longer
/// says where the bed is. It is dropped, and the user is told to run it again.
fn drop_stale_alignment(app: &mut DarkBurn) {
    if app.vision.state.camera.alignment.take().is_some() {
        app.vision.message = Some("The lens correction changed: run the camera alignment again.".into());
    }
}

fn camera_set(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let had_lens = app.vision.state.camera.lens.clone();
    let had_scale = app.vision.state.camera.focal_scale;
    {
        let c = &mut app.vision.state.camera;
        if let Some(v) = flag(a, "show") {
            c.state.show = v;
        }
        if let Some(v) = num(a, "fade") {
            c.state.fade = v.clamp(0.0, 1.0) as f32;
        }
        if let Some(v) = flag(a, "color").or_else(|| flag(a, "colour")) {
            c.state.color = v;
        }
        if let Some(v) = flag(a, "crop_to_page") {
            c.crop_to_page = v;
        }
        if let Some(v) = num(a, "x_shift") {
            c.overlay.x_shift_mm = v;
        }
        if let Some(v) = num(a, "y_shift") {
            c.overlay.y_shift_mm = v;
        }
        if let Some(v) = text(a, "kind") {
            c.kind = match v {
                "overhead" => CameraKind::Overhead,
                "head_mounted" | "head" => CameraKind::HeadMounted,
                other => return Err(format!("unknown camera type {other}")),
            };
        }
        if let Some(v) = text(a, "camera_name") {
            c.camera_name = v.to_string();
        }
        if let Some(v) = a.get("device_index") {
            c.device_index = v.as_u64().map(|i| i as u32);
        }
        if let Some(v) = num(a, "focal_scale") {
            if !(0.1..=10.0).contains(&v) {
                return Err("focal_scale must be between 0.1 and 10".into());
            }
            c.focal_scale = v;
        }
        if a.get("exposure").is_some() {
            c.exposure = num(a, "exposure");
        }
        if a.get("brightness").is_some() {
            c.brightness = num(a, "brightness");
        }
    }
    // Width and height are percent of the area the picture covers.
    let covers = app.vision.overlay.as_ref().map(|o| o.covers).unwrap_or_else(|| bed(app));
    for (key, is_w) in [("width_pct", true), ("height_pct", false)] {
        if let Some(v) = a.get(key) {
            let mm = v.as_f64().map(|p| {
                if p <= 0.0 {
                    return Err("width and height must be above zero".to_string());
                }
                Ok(p / 100.0 * if is_w { covers.width } else { covers.height })
            });
            let mm = mm.transpose()?;
            if is_w {
                app.vision.state.camera.overlay.width_mm = mm;
            } else {
                app.vision.state.camera.overlay.height_mm = mm;
            }
        }
    }
    if let Some(src) = app.vision.source.as_mut() {
        if let Some(e) = num(a, "exposure") {
            let _ = src.set_exposure(e);
        }
        if let Some(b) = num(a, "brightness") {
            let _ = src.set_brightness(b);
        }
    }
    if app.vision.state.camera.lens.is_some() && app.vision.state.camera.focal_scale != had_scale {
        drop_stale_alignment(app);
    }
    commit(app, "Camera settings", false);
    let rebuild =
        a.get("crop_to_page").is_some() || a.get("kind").is_some() || a.get("focal_scale").is_some() || had_lens != app.vision.state.camera.lens;
    if rebuild && app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    Ok(camera_json(app))
}

/// Save Settings [CAMP-05, CAM-09]: remember these camera settings (and the last camera) for new projects.
pub fn save_settings(app: &mut DarkBurn) -> Result<Value, String> {
    sync(app);
    let c = app.vision.state.camera.clone();
    app.vision.prefs.last_camera_name = c.camera_name.clone();
    app.vision.prefs.last_device_index = c.device_index;
    app.vision.prefs.saved_camera = Some(c);
    save_prefs(app)?;
    Ok(json!({ "saved": true }))
}

fn export_settings(app: &mut DarkBurn, path: &Path) -> Result<Value, String> {
    sync(app);
    std::fs::write(path, app.vision.state.camera.to_json()).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(json!({ "path": path.display().to_string() }))
}

fn import_settings(app: &mut DarkBurn, path: &Path) -> Result<Value, String> {
    sync(app);
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let c = camera::CaptureSettings::from_json(&text).map_err(|e| format!("not a camera settings file: {e}"))?;
    app.vision.state.camera = c;
    commit(app, "Import camera settings", true);
    if app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    Ok(camera_json(app))
}

// ----- Lens calibration -------------------------------------------------------------------------

/// The pattern the lens wizard looks for.
pub fn pattern_from(a: &Value) -> Result<PatternSpec, String> {
    let p = a.get("pattern").unwrap_or(a);
    let kind = match text(p, "kind").unwrap_or("circle_grid") {
        "circle_grid" | "circles" => PatternKind::CircleGrid { dark_on_light: flag(p, "dark_on_light").unwrap_or(true) },
        "checkerboard" => PatternKind::Checkerboard,
        other => return Err(format!("unknown pattern {other}")),
    };
    let dim = |k: &str, d: usize| p.get(k).and_then(Value::as_u64).map(|v| v as usize).unwrap_or(d);
    let spec = PatternSpec { kind, cols: dim("cols", 7), rows: dim("rows", 5), spacing_mm: num(p, "spacing_mm").unwrap_or(25.0) };
    if spec.cols < 2 || spec.rows < 2 || spec.cols > 40 || spec.rows > 40 || !(spec.spacing_mm > 0.0) {
        return Err("the pattern needs at least 2 by 2 points with a spacing above zero".into());
    }
    Ok(spec)
}

fn gray_of(img: &RgbaImage) -> GrayImage {
    camera::to_grayscale(img)
}

/// Add the current frame to the wizard's pictures if it shows the pattern. Returns how many are held.
pub fn calib_add_frame(app: &mut DarkBurn, spec: &PatternSpec) -> Result<usize, String> {
    let raw = app.vision.raw.as_ref().ok_or("there is no camera picture yet")?;
    let g = gray_of(raw);
    if darkburn_vision::detect::detect_pattern(&g, spec).is_none() {
        return Err("the pattern was not found in the picture: check the focus, light and that the whole target is in view".into());
    }
    if let Some(first) = app.vision.calib_frames.first()
        && first.dimensions() != g.dimensions()
    {
        return Err("every calibration picture must be the same size".into());
    }
    app.vision.calib_frames.push(g);
    Ok(app.vision.calib_frames.len())
}

/// Solve the lens from the collected pictures and keep it in the camera settings [LT-07, CAM-03].
pub fn calibrate_lens(app: &mut DarkBurn, spec: &PatternSpec, fisheye: bool, num_distortion: usize) -> Result<Value, String> {
    sync(app);
    let opts = CalibOptions {
        kind: if fisheye { LensKind::Fisheye } else { LensKind::Standard },
        num_distortion: num_distortion.clamp(1, if fisheye { 4 } else { 3 }),
        ..CalibOptions::default()
    };
    let (mut result, used) = calibrate_from_images(spec, &app.vision.calib_frames, opts).map_err(|e| e.to_string())?;
    result.model.name = app.vision.state.camera.camera_name.clone();
    let rms = result.rms_px;
    if app.vision.state.camera.lens.as_ref() != Some(&result.model) {
        drop_stale_alignment(app);
    }
    app.vision.state.camera.lens = Some(result.model);
    commit(app, "Lens calibration", true);
    app.vision.frame_gen += 1;
    app.vision.calib_result = Some(format!("Calibrated from {} picture(s), error {rms:.2} px.", used.len()));
    if app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    Ok(json!({ "rms_px": rms, "views_used": used.len(), "views_given": app.vision.calib_frames.len(), "fisheye": fisheye }))
}

fn lens_calibrate_cmd(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let spec = pattern_from(a)?;
    if let Some(files) = a.get("images").and_then(Value::as_array) {
        app.vision.calib_frames.clear();
        for f in files {
            let p = f.as_str().ok_or("images must be file paths")?;
            let bytes = std::fs::read(p).map_err(|e| format!("could not read {p}: {e}"))?;
            let img = crate::images::decode(&bytes)?.to_rgba8();
            app.vision.calib_frames.push(gray_of(&img));
        }
    }
    if app.vision.calib_frames.is_empty() {
        return Err("give the pictures of the pattern as images (file paths)".into());
    }
    calibrate_lens(app, &spec, flag(a, "fisheye").unwrap_or(false), a.get("num_distortion").and_then(Value::as_u64).unwrap_or(2) as usize)
}

pub fn clear_lens(app: &mut DarkBurn) -> Value {
    sync(app);
    if app.vision.state.camera.lens.take().is_some() {
        drop_stale_alignment(app);
    }
    app.vision.frame_gen += 1;
    app.vision.calib_frames.clear();
    app.vision.calib_result = None;
    commit(app, "Clear lens calibration", true);
    if app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    json!({ "calibrated": false })
}

// ----- Camera alignment -------------------------------------------------------------------------

/// Where the alignment targets go: `count` marks inset by `margin` from the bed edge.
pub fn align_positions(app: &DarkBurn, margin: f64, count: usize) -> Vec<Point> {
    default_targets(bed(app), margin, count)
}

fn align_generate(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let margin = num(a, "margin").unwrap_or(20.0).max(0.0);
    let count = a.get("count").and_then(Value::as_u64).unwrap_or(4).clamp(4, 9) as usize;
    let size = num(a, "size").unwrap_or(8.0);
    let layer = layer_arg(app, a, "layer", app.editor.current_layer)?;
    let pos = align_positions(app, margin, count);
    app.vision.align_margin = margin;
    app.vision.align_count = count;
    let mut out = None;
    let mut err = None;
    app.editor.edit("Camera alignment targets", |d, sel| match vision_tools::alignment_targets(d, &pos, size, layer) {
        Ok(g) => {
            sel.set(g.shapes.iter().copied());
            out = Some(g);
            true
        }
        Err(e) => {
            err = Some(e);
            false
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    let g = out.ok_or("nothing was drawn")?;
    Ok(json!({ "targets": pos, "created": g.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "layer": layer }))
}

/// Solve the alignment from the clicked target positions (pixels in the lens-corrected picture).
pub fn align_solve(
    app: &mut DarkBurn,
    clicks: &[Point],
    bed_points: Option<Vec<Point>>,
    orientation: ImageOrientation,
    size: (u32, u32),
) -> Result<Value, String> {
    sync(app);
    let bed_pts = match bed_points {
        Some(b) => b,
        None => {
            let (margin, count) = (if app.vision.align_margin > 0.0 { app.vision.align_margin } else { 20.0 }, clicks.len().max(4));
            align_positions(app, margin, count)
        }
    };
    if bed_pts.len() < clicks.len() {
        return Err("there are more clicked points than target positions".into());
    }
    let targets: Vec<AlignmentTarget> = clicks.iter().zip(&bed_pts).map(|(i, b)| AlignmentTarget { image: *i, bed: *b }).collect();
    let al = CameraAlignment::solve(&targets, orientation, size).map_err(|e| e.to_string())?;
    let rms = al.rms_mm;
    let residuals = al.residuals_mm(&targets);
    app.vision.state.camera.alignment = Some(al);
    app.vision.state.camera.kind = CameraKind::Overhead;
    commit(app, "Camera alignment", true);
    if app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    Ok(json!({ "rms_mm": rms, "residuals_mm": residuals }))
}

fn align_solve_cmd(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let clicks = points(a, "points")?;
    let bed_points = if a.get("bed").is_some() { Some(points(a, "bed")?) } else { None };
    if let Some(m) = num(a, "margin") {
        app.vision.align_margin = m;
    }
    let size = match (a.get("image_width").and_then(Value::as_u64), a.get("image_height").and_then(Value::as_u64)) {
        (Some(w), Some(h)) => (w as u32, h as u32),
        _ => {
            let r = undistorted(app).ok_or("there is no camera picture: load one or give image_width and image_height")?;
            (r.width(), r.height())
        }
    };
    let o = ImageOrientation {
        flip_x: flag(a, "flip_x").unwrap_or(false),
        flip_y: flag(a, "flip_y").unwrap_or(false),
        rotate_quarters: a.get("rotate_quarters").and_then(Value::as_u64).unwrap_or(0).min(3) as u8,
    };
    align_solve(app, &clicks, bed_points, o, size)
}

fn align_clear(app: &mut DarkBurn) -> Value {
    sync(app);
    app.vision.state.camera.alignment = None;
    app.vision.state.camera.head = None;
    commit(app, "Clear camera alignment", true);
    if app.vision.raw.is_some() {
        let _ = rebuild_overlay(app);
    }
    json!({ "aligned": false })
}

// ----- Head-mounted camera ----------------------------------------------------------------------

/// A mark holds where the head stood as the controller reports it (machine coordinates, like
/// every laser position here) and where the mark is in the design. The picture is placed in
/// design coordinates, so the head positions are turned to the design first.
pub fn head_solve(app: &mut DarkBurn, marks: Vec<HeadMark>, size: (u32, u32)) -> Result<Value, String> {
    sync(app);
    let marks: Vec<HeadMark> = marks.into_iter().map(|m| HeadMark { head_pos: laser_to_design(app, m.head_pos), ..m }).collect();
    let cam = &app.vision.state.camera;
    let cal = HeadCameraCalibration::solve(&marks, size, cam.lens.clone(), cam.focal_scale).map_err(|e| e.to_string())?;
    let rms = cal.rms_mm;
    app.vision.state.camera.head = Some(cal);
    app.vision.state.camera.kind = CameraKind::HeadMounted;
    commit(app, "Head camera alignment", true);
    Ok(json!({ "rms_mm": rms }))
}

fn head_solve_cmd(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    let list = a.get("marks").and_then(Value::as_array).ok_or("argument marks must be a list")?;
    let mut marks = Vec::new();
    for m in list {
        marks.push(HeadMark {
            image: point_of(m.get("image").ok_or("a mark needs image")?)?,
            head_pos: point_of(m.get("head").ok_or("a mark needs head")?)?,
            bed: point_of(m.get("bed").ok_or("a mark needs bed")?)?,
        });
    }
    let size = match (a.get("image_width").and_then(Value::as_u64), a.get("image_height").and_then(Value::as_u64)) {
        (Some(w), Some(h)) => (w as u32, h as u32),
        _ => app.vision.raw.as_ref().map(|r| (r.width(), r.height())).ok_or("give image_width and image_height")?,
    };
    head_solve(app, marks, size)
}

/// A head camera mark from a click in the picture the wizard shows. That picture is already
/// lens-corrected ([`undistorted`]); a mark holds the pixel of the camera's own picture, and the
/// solver and the stitcher do the lens correction themselves.
pub fn head_mark(app: &DarkBurn, click: Point, head: Point, bed: Point) -> HeadMark {
    let cam = &app.vision.state.camera;
    let image = match view_lens(app) {
        Some(l) => l.distort_point_with(click, cam.focal_scale),
        None => click,
    };
    HeadMark { image, head_pos: head, bed }
}

/// The lens [`undistorted`] corrects the current picture with, if it does.
fn view_lens(app: &DarkBurn) -> Option<&darkburn_vision::lens::LensModel> {
    let raw = app.vision.raw.as_ref()?;
    app.vision.state.camera.lens.as_ref().filter(|l| l.width == raw.width() && l.height == raw.height())
}

/// Where a mark's pixel is in the lens-corrected picture the wizard shows.
pub fn mark_in_view(app: &DarkBurn, mark: &HeadMark) -> Point {
    match view_lens(app) {
        Some(l) => l.undistort_point_with(mark.image, app.vision.state.camera.focal_scale),
        None => mark.image,
    }
}

/// Where the head is for a head camera mark or capture [CAM-05]: `typed` when given, else where
/// the connected laser reports it (machine coordinates, from machine zero).
pub fn head_now(app: &DarkBurn, typed: Option<Point>) -> Result<Point, String> {
    match typed {
        Some(p) => Ok(p),
        None if app.machine.handle.is_connected() => crate::motion_cmds::head_target(app, false),
        None => Err("connect the laser to read where the head is, or give the head position".into()),
    }
}

/// `head` is where the controller reports the head (machine coordinates).
pub fn head_add_capture(app: &mut DarkBurn, img: RgbaImage, head: Point) -> usize {
    let head = laser_to_design(app, head);
    app.vision.mosaic.push(MosaicCapture { head_pos: head, image: img });
    app.vision.mosaic.len()
}

/// Stitch the head camera captures into the overlay [CAM-05].
pub fn head_stitch(app: &mut DarkBurn) -> Result<Value, String> {
    sync(app);
    let cal = app.vision.state.camera.head.clone().ok_or("align the head camera first")?;
    if app.vision.mosaic.is_empty() {
        return Err("add captures taken at different head positions first".into());
    }
    let b = bed(app);
    let ppm = (1600.0 / b.width.max(b.height)).clamp(0.25, 2.0);
    let img = camera::stitch_mosaic(&app.vision.mosaic, &cal, b, ppm);
    let w = img.width();
    app.vision.overlay = Some(Overlay { img, covers: b });
    app.vision.overlay_gen += 1;
    Ok(json!({ "captures": app.vision.mosaic.len(), "width_px": w }))
}

// ----- Rotary and cylinder ---------------------------------------------------------------------

fn rotary_json(app: &DarkBurn) -> Value {
    let r = &app.vision.state.rotary;
    json!({ "rotary": r, "split": app.vision.state.rotary_split, "valid": r.validate().is_ok() || !r.enabled })
}

pub fn rotary_set(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let (rotary, split) = rotary_with(app, a)?;
    app.vision.state.rotary = rotary;
    app.vision.state.rotary_split = split;
    commit(app, "Rotary setup", true);
    Ok(rotary_json(app))
}

/// The test rotation the settings `a` would give, without keeping them: the Rotary Setup window
/// plans from what it shows, and only its Apply button changes the project.
pub fn rotary_plan_for(app: &mut DarkBurn, a: &Value, revolutions: f64) -> Result<Value, String> {
    sync(app);
    if revolutions == 0.0 {
        return Err("revolutions must not be zero".into());
    }
    let (mut r, _) = rotary_with(app, a)?;
    r.enabled = true;
    let plan = rotary::test_rotation_plan(&r, revolutions).map_err(|e| e.to_string())?;
    Ok(json!({ "plan": plan, "sent": false, "note": "This is the plan only; the machine side sends it." }))
}

/// The project's rotary setup and split settings with the changes `a` asks for.
fn rotary_with(app: &DarkBurn, a: &Value) -> Result<(darkburn_vision::rotary::RotaryParams, crate::vision_state::SplitSettings), String> {
    let mut r = app.vision.state.rotary;
    if let Some(v) = flag(a, "enabled") {
        r.enabled = v;
    }
    if let Some(v) = text(a, "kind") {
        r.kind = match v {
            "chuck" => RotaryKind::Chuck,
            "roller" => RotaryKind::Roller,
            other => return Err(format!("unknown rotary type {other}")),
        };
    }
    if let Some(v) = text(a, "axis") {
        r.axis = RotaryAxis::all().into_iter().find(|x| x.letter().to_string().eq_ignore_ascii_case(v)).ok_or_else(|| format!("unknown axis {v}"))?;
    }
    if let Some(v) = num(a, "steps_per_rotation") {
        r.steps_per_rotation = v;
    }
    if let Some(v) = num(a, "axis_steps_per_mm") {
        r.axis_steps_per_mm = v;
    }
    if let Some(v) = num(a, "roller_diameter") {
        r.roller_diameter = v;
    }
    if let Some(v) = num(a, "diameter") {
        r.object = ObjectSize::Diameter(v);
    }
    if let Some(v) = num(a, "circumference") {
        r.object = ObjectSize::Circumference(v);
    }
    if let Some(v) = flag(a, "mirror") {
        r.mirror = v;
    }
    if r.enabled {
        r.validate().map_err(|e| e.to_string())?;
    }
    let mut split = app.vision.state.rotary_split;
    if let Some(m) = text(a, "split") {
        split.mode = match m {
            "off" => SplitMode::Off,
            "slice" => SplitMode::Slice,
            "shape" => SplitMode::Shape,
            other => return Err(format!("unknown split mode {other}")),
        };
    }
    if let Some(v) = num(a, "split_max_size") {
        if !(v > 0.0) {
            return Err("the split size must be above zero".into());
        }
        split.max_size = v;
    }
    Ok((r, split))
}

fn rotary_test_plan(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let revs = num(a, "revolutions").unwrap_or(1.0);
    if revs == 0.0 {
        return Err("revolutions must not be zero".into());
    }
    let mut r = app.vision.state.rotary;
    r.enabled = true;
    let plan = rotary::test_rotation_plan(&r, revs).map_err(|e| e.to_string())?;
    Ok(json!({ "plan": plan, "sent": false, "note": "This is the plan only; the machine side sends it." }))
}

/// The selection (or the whole design) split for galvo rotary marking; data only.
pub fn rotary_split(app: &mut DarkBurn) -> Result<Value, String> {
    sync(app);
    let s = app.vision.state.rotary_split;
    let mut r = app.vision.state.rotary;
    r.enabled = true;
    let ids = app.editor.selected();
    let paths: Vec<_> = if ids.is_empty() {
        app.editor.doc.iter().map(|s| s.outline()).collect()
    } else {
        ids.iter().filter_map(|i| app.editor.doc.get(*i)).map(|s| s.outline()).collect()
    };
    if paths.is_empty() {
        return Err("there is nothing to split".into());
    }
    let slices = match s.mode {
        SplitMode::Off => return Err("turn on splitting by slice or by shape first".into()),
        SplitMode::Slice => rotary::split_by_slice(&paths, s.max_size, &r),
        SplitMode::Shape => rotary::split_by_shape(&paths, s.max_size, &r),
    }
    .map_err(|e| e.to_string())?;
    Ok(json!({
        "mode": s.mode,
        "slices": slices.iter().map(|sl| json!({
            "index": sl.index, "y_min": sl.y_min, "y_max": sl.y_max, "rotation_deg": sl.rotation_deg, "paths": sl.paths.len(), "cut_across": sl.cut_across,
        })).collect::<Vec<_>>(),
    }))
}

pub fn cylinder_set(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let mut c = app.vision.state.cylinder;
    if let Some(v) = flag(a, "enabled") {
        c.enabled = v;
    }
    if let Some(v) = num(a, "radius") {
        c.params.radius = v;
    }
    if let Some(v) = num(a, "diameter") {
        c.params.radius = v / 2.0;
    }
    if let Some(v) = num(a, "centre").or_else(|| num(a, "center")) {
        c.params.centre = v;
    }
    if let Some(v) = text(a, "kind") {
        c.params.kind = match v {
            "convex" => CylinderKind::Convex,
            "concave" => CylinderKind::Concave,
            other => return Err(format!("unknown cylinder type {other}")),
        };
    }
    if let Some(v) = text(a, "axis") {
        c.params.axis = match v {
            "along_x" | "x" => CylinderAxis::AlongX,
            "along_y" | "y" => CylinderAxis::AlongY,
            other => return Err(format!("unknown cylinder axis {other}")),
        };
    }
    if !(c.params.radius > 0.0) {
        return Err("the radius must be above zero".into());
    }
    app.vision.state.cylinder = c;
    commit(app, "Cylinder correction", true);
    Ok(json!({ "cylinder": app.vision.state.cylinder }))
}

// ----- Print and cut ---------------------------------------------------------------------------

fn corner(r: Rect, index: usize) -> Point {
    match index % 4 {
        0 => Point::new(r.x0, r.y0),
        1 => Point::new(r.x1, r.y0),
        2 => Point::new(r.x1, r.y1),
        _ => Point::new(r.x0, r.y1),
    }
}

/// Design to machine coordinates for the selected device (origin corner, negative axes, swapped axes).
fn machine_affine(app: &DarkBurn) -> darkburn_core::kurbo::Affine {
    let mut device = app.machine.active().job_device();
    device.swap_xy |= app.job.gcode.swap_xy;
    device.geometry().affine()
}

/// The design position under the laser when the controller reports `laser`. Laser positions are
/// machine coordinates; the registration is fitted and applied in design coordinates, before the
/// job is turned to the machine's origin corner.
pub fn laser_to_design(app: &DarkBurn, laser: Point) -> Point {
    machine_affine(app).inverse() * laser
}

/// Set target `index` (0 first, 1 second): `laser` is where the laser stands on it [LT-01, LT-02].
/// The design position defaults to a corner of the selection.
pub fn pnc_set_target(app: &mut DarkBurn, index: usize, laser: Point, design: Option<Point>) -> Result<Value, String> {
    sync(app);
    let design = match design {
        Some(d) => Some(d),
        None if app.vision.state.pnc.design.get(index).copied().flatten().is_some() => None,
        None => {
            let sel = app.editor.selected();
            let b = app.editor.doc.bbox_of(&sel).ok_or("select the artwork the targets belong to, or give design_x and design_y")?;
            Some(corner(b, index))
        }
    };
    app.vision.state.pnc.set_target(index, design, laser);
    commit(app, if index == 0 { "Set first target" } else { "Set second target" }, true);
    let p = &app.vision.state.pnc;
    Ok(json!({ "index": index, "design": p.design[index], "laser": p.measured[index] }))
}

/// Align output to the targets, with or without scaling [LT-03, LT-04].
pub fn pnc_align(app: &mut DarkBurn, mode: PnCMode) -> Result<Value, String> {
    sync(app);
    let (d, m) = app.vision.state.pnc.complete_pairs();
    if d.len() < 2 {
        return Err("set the first and second target first".into());
    }
    let m: Vec<Point> = m.iter().map(|p| laser_to_design(app, *p)).collect();
    let fit = darkburn_vision::printcut::fit_registration(&d, &m, mode).map_err(|e| e.to_string())?;
    let report = json!({
        "mode": mode, "scale": fit.scale, "rotation_deg": fit.rotation_deg, "translation": fit.translation,
        "rms_mm": fit.residuals.rms_mm, "max_mm": fit.residuals.max_mm,
    });
    app.vision.state.pnc.mode = mode;
    app.vision.state.pnc.fit = Some(fit);
    commit(app, "Align to targets", true);
    Ok(report)
}

pub fn pnc_reset(app: &mut DarkBurn) -> Value {
    sync(app);
    app.vision.state.pnc.reset();
    commit(app, "Reset print and cut", true);
    json!({ "active": false })
}

fn wizard_summary(app: &DarkBurn) -> Value {
    match &app.vision.state.pnc.wizard {
        None => json!({ "wizard": null }),
        Some(w) => json!({
            // the wizard keeps design positions; shown as the laser positions that were entered
            "step": w.step, "mode": w.mode, "measured": w.measured.iter().map(|p| machine_affine(app) * *p).collect::<Vec<_>>(), "current_target": w.current_target(),
            "targets": w.design_targets, "active": w.active_fit().is_some(),
        }),
    }
}

/// Feed the wizard an event and keep its state. A confirmed fit becomes the active registration [LT-06, PNC-05].
pub fn pnc_wizard_event(app: &mut DarkBurn, event: PnCEvent) -> Result<Value, String> {
    sync(app);
    let mut w = app.vision.state.pnc.wizard.clone().ok_or("start the wizard first")?;
    let event = match event {
        PnCEvent::Measured(laser) => PnCEvent::Measured(laser_to_design(app, laser)),
        other => other,
    };
    w.handle(event).map_err(|e| e.to_string())?;
    if let Some(f) = w.active_fit() {
        app.vision.state.pnc.fit = Some(f.clone());
        app.vision.state.pnc.mode = w.mode;
        app.vision.state.pnc.design = w.design_targets.iter().map(|p| Some(*p)).collect();
        let to_machine = machine_affine(app);
        app.vision.state.pnc.measured = w.measured.iter().map(|p| Some(to_machine * *p)).collect();
    }
    if matches!(event, PnCEvent::Reset) {
        app.vision.state.pnc.fit = None;
    }
    app.vision.state.pnc.wizard = Some(w);
    commit(app, "Print and cut wizard", true);
    Ok(wizard_summary(app))
}

pub fn pnc_wizard_start(app: &mut DarkBurn, targets: Option<Vec<Point>>) -> Result<Value, String> {
    sync(app);
    let targets = match targets {
        Some(t) => t,
        None => {
            let sel = app.editor.selected();
            match app.editor.doc.bbox_of(&sel) {
                Some(b) => vec![corner(b, 0), corner(b, 1)],
                None => app.vision.state.pnc.design.iter().flatten().copied().collect(),
            }
        }
    };
    let w = PnCWizard::new(targets).map_err(|e| e.to_string())?;
    app.vision.state.pnc.wizard = Some(w);
    commit(app, "Start print and cut wizard", true);
    Ok(wizard_summary(app))
}

fn wizard_cmd(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    if a.get("start").and_then(Value::as_bool).unwrap_or(false) {
        let t = if a.get("targets").is_some() { Some(points(a, "targets")?) } else { None };
        return pnc_wizard_start(app, t);
    }
    let Some(ev) = text(a, "event") else {
        sync(app);
        return Ok(wizard_summary(app));
    };
    let event = match ev {
        "select_mode" => PnCEvent::SelectMode(match text(a, "mode").unwrap_or("scaled") {
            "scaled" => PnCMode::Scaled,
            "unscaled" => PnCMode::Unscaled,
            "full_affine" => PnCMode::FullAffine,
            other => return Err(format!("unknown mode {other}")),
        }),
        "measured" => PnCEvent::Measured(point_of(a)?),
        "back" => PnCEvent::Back,
        "confirm" => PnCEvent::Confirm,
        "reset" => PnCEvent::Reset,
        "cancel" => PnCEvent::Cancel,
        other => return Err(format!("unknown wizard event {other}")),
    };
    pnc_wizard_event(app, event)
}

/// One line for the main window: what print and cut is doing [PNC-06].
pub fn pnc_status(state: &VisionState) -> Option<String> {
    use darkburn_vision::printcut::PnCStatus;
    let mut w = state.pnc.wizard.clone();
    let from_wizard = w.as_mut().map(|w| w.status());
    let set = state.pnc.measured.iter().flatten().count();
    match (&state.pnc.fit, from_wizard) {
        (Some(f), _) => Some(format!("Print and cut: on, {}, error {:.2} mm", mode_name(f.mode), f.residuals.rms_mm)),
        (None, Some(PnCStatus::InProgress { done, total })) => Some(format!("Print and cut: {done} of {total} targets")),
        _ if set > 0 => Some(format!("Print and cut: {set} target(s) set")),
        _ => None,
    }
}

fn mode_name(m: PnCMode) -> &'static str {
    match m {
        PnCMode::Scaled => "with scaling",
        PnCMode::Unscaled => "no scaling",
        PnCMode::FullAffine => "full fit",
    }
}

/// The modes that are on, for the status bar and the Modes toolbar tooltip [ROT-08, LASER-22].
pub fn mode_status(state: &VisionState) -> String {
    let mut parts = Vec::new();
    if state.rotary.enabled {
        parts.push(format!("Rotary: {} {}", state.rotary.axis.letter(), if state.rotary.kind == RotaryKind::Roller { "roller" } else { "chuck" }));
    }
    if state.cylinder.enabled {
        parts.push(format!("Cylinder: {}", if state.cylinder.params.kind == CylinderKind::Concave { "concave" } else { "convex" }));
    }
    if let Some(p) = pnc_status(state) {
        parts.push(p);
    }
    parts.join("  |  ")
}

pub fn pnc_json(app: &DarkBurn) -> Value {
    let p = &app.vision.state.pnc;
    json!({ "modes": mode_status(&app.vision.state), "design": p.design, "laser": p.measured, "mode": p.mode, "active": p.fit.is_some(), "status": pnc_status(&app.vision.state), "fit": p.fit.as_ref().map(|f| json!({
        "scale": f.scale, "rotation_deg": f.rotation_deg, "translation": f.translation, "rms_mm": f.residuals.rms_mm })) })
}

// ----- Test tools ------------------------------------------------------------------------------

fn at_of(a: &Value) -> Point {
    Point::new(num(a, "x").unwrap_or(10.0), num(a, "y").unwrap_or(10.0))
}

fn generated(g: &vision_tools::Generated) -> Value {
    json!({ "created": g.shapes.iter().map(|s| s.0).collect::<Vec<_>>(), "layers": g.layers, "bounds": [g.bounds.x0, g.bounds.y0, g.bounds.x1, g.bounds.y1] })
}

fn run_generator(
    app: &mut DarkBurn,
    label: &str,
    f: impl FnOnce(&mut darkburn_core::Document) -> Result<vision_tools::Generated, String>,
) -> Result<Value, String> {
    let mut out = None;
    let mut err = None;
    app.editor.edit(label, |d, sel| match f(d) {
        Ok(g) => {
            sel.set(g.shapes.iter().copied());
            out = Some(g);
            true
        }
        Err(e) => {
            err = Some(e);
            false
        }
    });
    match (out, err) {
        (Some(g), _) => Ok(generated(&g)),
        (_, Some(e)) => Err(e),
        _ => Err("nothing was drawn".into()),
    }
}

pub fn focus_generate(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let mut p = app.vision.state.focus;
    if let Some(v) = num(a, "z_start") {
        p.z_start = v;
    }
    if let Some(v) = num(a, "z_end") {
        p.z_end = v;
    }
    if let Some(v) = a.get("steps").and_then(Value::as_u64) {
        p.steps = v as usize;
    }
    if let Some(v) = num(a, "line_length") {
        p.line_length = v;
    }
    if let Some(v) = num(a, "row_pitch") {
        p.row_pitch = v;
    }
    if let Some(v) = num(a, "text_height") {
        p.text_height = v;
    }
    if let Some(v) = flag(a, "labels") {
        p.with_labels = v;
    }
    let first = layer_arg(app, a, "first_layer", 0)?;
    let labels = layer_arg(app, a, "label_layer", 29)?;
    let at = at_of(a);
    let r = run_generator(app, "Focus test", |d| vision_tools::focus(d, &p, at, first, labels))?;
    app.vision.state.focus = p;
    commit(app, "Focus test settings", false);
    Ok(r)
}

pub fn interval_generate(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let mut p = app.vision.state.interval;
    if let Some(v) = num(a, "interval_start") {
        p.interval_start = v;
    }
    if let Some(v) = num(a, "interval_end") {
        p.interval_end = v;
    }
    if let Some(v) = a.get("steps").and_then(Value::as_u64) {
        p.steps = v as usize;
    }
    if let Some(v) = num(a, "block_width") {
        p.block_width = v;
    }
    if let Some(v) = num(a, "block_height") {
        p.block_height = v;
    }
    if let Some(v) = num(a, "gap") {
        p.gap = v;
    }
    if let Some(v) = flag(a, "labels") {
        p.with_labels = v;
    }
    let first = layer_arg(app, a, "first_layer", 0)?;
    let labels = layer_arg(app, a, "label_layer", 29)?;
    let at = at_of(a);
    let r = run_generator(app, "Interval test", |d| vision_tools::interval(d, &p, at, first, labels))?;
    app.vision.state.interval = p;
    commit(app, "Interval test settings", false);
    Ok(r)
}

/// Centre and diameter of round stock from edge points the laser measured [LT-13, OTHER-03].
pub fn centre_find(app: &mut DarkBurn, pts: &[Point], mark: bool, layer: usize) -> Result<Value, String> {
    let fit = fit_circle(pts).map_err(|e| e.to_string())?;
    let mut created = Vec::new();
    if mark {
        // The points are laser positions; the mark goes where that spot is in the design.
        let c = laser_to_design(app, fit.centre);
        let path = {
            let mut p = darkburn_core::kurbo::Circle::new(c, fit.radius).to_path(0.05);
            p.extend(vision_tools::target_mark(c, (fit.radius * 0.3).clamp(2.0, 10.0)));
            p
        };
        app.editor.edit("Centre finder", |d, sel| {
            let mut s = darkburn_core::Shape::path(path, layer);
            s.name = "Centre mark".into();
            let id = d.add(s);
            sel.set([id]);
            created.push(id.0);
            true
        });
    }
    Ok(json!({ "centre": fit.centre, "diameter": fit.diameter, "radius": fit.radius, "rms_mm": fit.rms_mm, "created": created }))
}

/// Feeder numbers: window plan, corrected steps per mm, roller feed and delay [LT-11, OTHER-04].
pub fn feeder_calc(app: &mut DarkBurn, a: &Value) -> Result<Value, String> {
    sync(app);
    let mut f = app.vision.state.feeder;
    for (k, slot) in [
        ("window_height", &mut f.window_height),
        ("overlap", &mut f.overlap),
        ("total_length", &mut f.total_length),
        ("roller_diameter", &mut f.roller_diameter),
        ("steps_per_rotation", &mut f.steps_per_rotation),
        ("current_steps_per_mm", &mut f.current_steps_per_mm),
        ("commanded", &mut f.commanded),
        ("measured", &mut f.measured),
        ("speed_mm_s", &mut f.speed_mm_s),
        ("settle_ms", &mut f.settle_ms),
    ] {
        if let Some(v) = num(a, k) {
            *slot = v;
        }
    }
    let windows = vision_tools::feeder_plan(f.total_length, f.window_height, f.overlap)?;
    let advance = feeder::distance_per_feed(f.window_height, f.overlap).map_err(|e| e.to_string())?;
    let corrected = feeder::corrected_steps_per_mm(f.current_steps_per_mm, f.commanded, f.measured).map_err(|e| e.to_string())?;
    let steps = feeder::steps_for_roller_feed(advance, f.roller_diameter, f.steps_per_rotation).map_err(|e| e.to_string())?;
    let delay = feeder::feed_delay_ms(advance, f.speed_mm_s, f.settle_ms).map_err(|e| e.to_string())?;
    let mut created = Value::Null;
    if flag(a, "draw").unwrap_or(false) {
        let layer = layer_arg(app, a, "layer", 28)?;
        let width = num(a, "width").unwrap_or(100.0);
        let wh = f.window_height;
        created = run_generator(app, "Feeder windows", |d| vision_tools::feeder_windows(d, &windows, wh, width, layer))?;
    }
    app.vision.state.feeder = f;
    commit(app, "Feeder setup", false);
    Ok(json!({
        "windows": windows.len(), "advance_mm": advance, "corrected_steps_per_mm": corrected, "roller_feed_steps": steps,
        "feed_delay_ms": delay, "plan": windows, "drawn": created,
    }))
}

// ----- The registry ------------------------------------------------------------------------------

const fn a(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

/// Commands of this area that take arguments (the menu ones are in commands.rs).
pub const COMMANDS: &[Cmd] = &[
    a("camera.get", "Camera Settings and State", ""),
    a(
        "camera.set",
        "Change Camera Settings",
        r#"{"type":"object","properties":{"show":{"type":"boolean"},"fade":{"type":"number","description":"0 visible to 1 faded out"},"color":{"type":"boolean","description":"false is black and white"},"width_pct":{"type":["number","null"]},"height_pct":{"type":["number","null"]},"x_shift":{"type":"number","description":"mm"},"y_shift":{"type":"number","description":"mm"},"crop_to_page":{"type":"boolean"},"kind":{"enum":["overhead","head_mounted"]},"camera_name":{"type":"string"},"device_index":{"type":["integer","null"]},"exposure":{"type":["number","null"]},"brightness":{"type":["number","null"]},"focal_scale":{"type":"number"}}}"#,
    ),
    a(
        "camera.load_frame",
        "Use a Picture File as the Camera Image",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    ),
    a("camera.find", "Find Cameras", ""),
    a(
        "camera.start",
        "Start the Live Camera",
        r#"{"type":"object","properties":{"index":{"type":"integer","minimum":0,"description":"camera number from camera.find; default the chosen camera"}}}"#,
    ),
    a("camera.stop", "Stop the Live Camera", ""),
    a("camera.update_overlay", "Update the Camera Overlay", ""),
    a(
        "camera.trace",
        "Trace the Camera Overlay",
        r#"{"type":"object","description":"Trace Image settings (threshold, ignore_less_than, ...), all optional"}"#,
    ),
    a(
        "camera.trace_picture",
        "Take a Camera Picture and Trace It",
        r#"{"type":"object","description":"Trace Image settings (threshold, ignore_smaller, ...), all optional; delete_image is on unless set false","properties":{"apply":{"type":"boolean","description":"trace at once, as one undo step (default false: open the Trace Image window on the picture)"},"delete_image":{"type":"boolean"}}}"#,
    ),
    a("camera.save_settings", "Save Camera Settings for New Projects", ""),
    a("camera.export_settings", "Export Camera Settings", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a("camera.import_settings", "Import Camera Settings", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a("camera.save_capture", "Save the Camera Picture to a File", r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#),
    a(
        "camera.lens_calibrate",
        "Calibrate the Lens from Pictures of a Pattern",
        r#"{"type":"object","properties":{"images":{"type":"array","items":{"type":"string"}},"pattern":{"type":"object","properties":{"kind":{"enum":["circle_grid","checkerboard"]},"cols":{"type":"integer"},"rows":{"type":"integer"},"spacing_mm":{"type":"number"},"dark_on_light":{"type":"boolean"}}},"fisheye":{"type":"boolean"},"num_distortion":{"type":"integer"}}}"#,
    ),
    a("camera.lens_clear", "Remove the Lens Calibration", ""),
    a(
        "camera.align_generate",
        "Draw the Camera Alignment Targets",
        r#"{"type":"object","properties":{"count":{"type":"integer","minimum":4,"maximum":9},"margin":{"type":"number"},"size":{"type":"number"},"layer":{"type":"integer"}}}"#,
    ),
    a(
        "camera.align_solve",
        "Align the Camera from Clicked Target Positions",
        r#"{"type":"object","properties":{"points":{"type":"array","items":{"type":"object"},"description":"pixel positions of the targets in the lens-corrected picture, in the order they were drawn"},"bed":{"type":"array","items":{"type":"object"},"description":"true target positions in mm; default the drawn ones"},"margin":{"type":"number"},"image_width":{"type":"integer"},"image_height":{"type":"integer"},"flip_x":{"type":"boolean"},"flip_y":{"type":"boolean"},"rotate_quarters":{"type":"integer"}},"required":["points"]}"#,
    ),
    a("camera.align_clear", "Remove the Camera Alignment", ""),
    a(
        "camera.head_solve",
        "Align a Head-Mounted Camera from Marks",
        r#"{"type":"object","properties":{"marks":{"type":"array","items":{"type":"object","properties":{"image":{"type":"object"},"head":{"type":"object"},"bed":{"type":"object"}}}},"image_width":{"type":"integer"},"image_height":{"type":"integer"}},"required":["marks"]}"#,
    ),
    a(
        "camera.head_add_capture",
        "Add a Head Camera Capture for Stitching",
        r#"{"type":"object","properties":{"path":{"type":"string"},"x":{"type":"number"},"y":{"type":"number"}}}"#,
    ),
    a("camera.head_stitch", "Stitch the Head Camera Captures", ""),
    a(
        "rotary.set",
        "Change Rotary Setup",
        r#"{"type":"object","properties":{"enabled":{"type":"boolean"},"kind":{"enum":["chuck","roller"]},"axis":{"enum":["Y","A","B","C","U","V"]},"steps_per_rotation":{"type":"number"},"axis_steps_per_mm":{"type":"number"},"roller_diameter":{"type":"number"},"diameter":{"type":"number"},"circumference":{"type":"number"},"mirror":{"type":"boolean"},"split":{"enum":["off","slice","shape"]},"split_max_size":{"type":"number"}}}"#,
    ),
    a("rotary.test_plan", "Plan a Rotary Test Rotation", r#"{"type":"object","properties":{"revolutions":{"type":"number"}}}"#),
    a("rotary.split", "Split the Design for Galvo Rotary Marking", ""),
    a(
        "cylinder.set",
        "Change Cylinder Correction",
        r#"{"type":"object","properties":{"enabled":{"type":"boolean"},"radius":{"type":"number"},"diameter":{"type":"number"},"kind":{"enum":["convex","concave"]},"axis":{"enum":["along_x","along_y"]},"centre":{"type":"number","description":"bed coordinate of the line nearest the beam, mm"}}}"#,
    ),
    a("pnc.get", "Print and Cut State", ""),
    a(
        "pnc.set_target",
        "Set a Print and Cut Target",
        r#"{"type":"object","properties":{"index":{"type":"integer","minimum":0,"maximum":3},"x":{"type":"number","description":"laser position, mm"},"y":{"type":"number"},"design_x":{"type":"number"},"design_y":{"type":"number"}},"required":["index","x","y"]}"#,
    ),
    a(
        "pnc.wizard",
        "Drive the Print and Cut Wizard",
        r#"{"type":"object","properties":{"start":{"type":"boolean"},"targets":{"type":"array","items":{"type":"object"}},"event":{"enum":["select_mode","measured","back","confirm","reset","cancel"]},"mode":{"enum":["scaled","unscaled","full_affine"]},"x":{"type":"number"},"y":{"type":"number"}}}"#,
    ),
    a(
        "test.focus",
        "Draw a Focus Test",
        r#"{"type":"object","properties":{"z_start":{"type":"number"},"z_end":{"type":"number"},"steps":{"type":"integer"},"line_length":{"type":"number"},"row_pitch":{"type":"number"},"text_height":{"type":"number"},"labels":{"type":"boolean"},"x":{"type":"number"},"y":{"type":"number"},"first_layer":{"type":"integer"},"label_layer":{"type":"integer"}}}"#,
    ),
    a(
        "test.interval",
        "Draw an Interval Test",
        r#"{"type":"object","properties":{"interval_start":{"type":"number"},"interval_end":{"type":"number"},"steps":{"type":"integer"},"block_width":{"type":"number"},"block_height":{"type":"number"},"gap":{"type":"number"},"labels":{"type":"boolean"},"x":{"type":"number"},"y":{"type":"number"},"first_layer":{"type":"integer"},"label_layer":{"type":"integer"}}}"#,
    ),
    a(
        "test.centre",
        "Find the Centre of Round Stock",
        r#"{"type":"object","properties":{"points":{"type":"array","items":{"type":"object"},"description":"three or more edge points, mm"},"mark":{"type":"boolean"},"layer":{"type":"integer"}},"required":["points"]}"#,
    ),
    a(
        "test.feeder",
        "Calculate Feeder Settings",
        r#"{"type":"object","properties":{"window_height":{"type":"number"},"overlap":{"type":"number"},"total_length":{"type":"number"},"roller_diameter":{"type":"number"},"steps_per_rotation":{"type":"number"},"current_steps_per_mm":{"type":"number"},"commanded":{"type":"number"},"measured":{"type":"number"},"speed_mm_s":{"type":"number"},"settle_ms":{"type":"number"},"draw":{"type":"boolean"},"width":{"type":"number"},"layer":{"type":"integer"}}}"#,
    ),
];

/// The menu commands that are the same as an argument command when run without a window.
fn canonical(id: &str) -> &str {
    match id {
        "laser.capture" => "camera.update_overlay",
        "file.save_capture" => "camera.save_capture",
        "laser.camera_lens" => "camera.lens_calibrate",
        "laser.camera_alignment" => "camera.align_solve",
        "laser.rotary" => "rotary.set",
        "laser.cylinder" => "cylinder.set",
        "laser.print_and_cut" => "pnc.get",
        "laser.pnc_wizard" => "pnc.wizard",
        "laser.focus_test" => "test.focus",
        "laser.interval_test" => "test.interval",
        "laser.centre_finder" => "test.centre",
        "laser.feeder" => "test.feeder",
        other => other,
    }
}

/// Every command this area runs (menu ones and the argument ones).
pub fn handles(id: &str) -> bool {
    matches!(
        id,
        "laser.pnc_first"
            | "laser.pnc_second"
            | "laser.pnc_align"
            | "laser.pnc_align_noscale"
            | "laser.pnc_reset"
            | "laser.pnc_wizard"
            | "laser.camera_lens"
            | "laser.camera_alignment"
            | "laser.rotary"
            | "laser.focus_test"
            | "laser.interval_test"
            | "laser.centre_finder"
            | "laser.feeder"
            | "laser.cylinder"
            | "laser.print_and_cut"
            | "laser.capture"
            | "file.save_capture"
    ) || COMMANDS.iter().any(|c| c.id == id)
}

/// The commands that open, list or close a camera: they talk to a USB device, so an agent must
/// be allowed hardware to run them.
pub fn hardware(id: &str) -> bool {
    matches!(id, "camera.find" | "camera.start" | "camera.stop")
}

/// The others move nothing and send nothing, so the `laser.` prefix rule does not apply.
pub fn not_hardware(id: &str) -> bool {
    handles(id) && !hardware(id)
}

pub fn works(id: &str) -> bool {
    handles(id)
}

pub fn checklist(id: &str) -> &'static [&'static str] {
    match id {
        "laser.pnc_first" => &["LT-01", "PNC-01"],
        "laser.pnc_second" => &["LT-02", "PNC-01"],
        "pnc.set_target" => &["LT-01", "LT-02", "PNC-01"],
        "laser.pnc_align" => &["LT-03", "PNC-02"],
        "laser.pnc_align_noscale" => &["LT-04", "PNC-03"],
        "laser.pnc_reset" => &["LT-05", "PNC-04"],
        "laser.pnc_wizard" | "pnc.wizard" => &["LT-06", "PNC-05"],
        "laser.print_and_cut" | "pnc.get" => &["PNC-06", "TB-23"],
        "laser.camera_lens" | "camera.lens_calibrate" | "camera.lens_clear" => &["LT-07", "CAM-03"],
        "laser.camera_alignment" | "camera.align_generate" | "camera.align_solve" | "camera.align_clear" => &["LT-08", "CAM-04"],
        "camera.head_solve" | "camera.head_add_capture" | "camera.head_stitch" => &["CAM-05"],
        "laser.rotary" | "rotary.set" | "rotary.split" => &["LT-09", "ROT-01", "ROT-02", "ROT-03", "ROT-04", "ROT-05", "ROT-08", "TB-23", "LASER-22"],
        "rotary.test_plan" => &["ROT-06"],
        "laser.cylinder" | "cylinder.set" => &["LT-15", "OTHER-05", "TB-23", "LASER-22"],
        "laser.focus_test" | "test.focus" => &["LT-10", "OTHER-01"],
        "laser.interval_test" | "test.interval" => &["LT-11", "OTHER-02"],
        "laser.centre_finder" | "test.centre" => &["LT-13", "OTHER-03"],
        "laser.feeder" | "test.feeder" => &["OTHER-04"],
        "laser.capture" | "camera.update_overlay" | "camera.load_frame" => &["CAMP-02", "CAMP-03", "CAM-06"],
        "camera.trace" => &["CAMP-04"],
        "camera.trace_picture" => &["IMG-20", "CAMP-04"],
        "camera.find" => &["CAMP-01", "CAM-01", "SET-37"],
        "camera.start" => &["CAMP-01", "CAMP-02", "CAM-01", "SET-37"],
        "camera.stop" => &["CAMP-02"],
        "camera.save_settings" => &["CAMP-05", "CAM-09"],
        "camera.set" | "camera.get" => &["CAMP-06", "CAMP-07", "CAM-01", "CAM-02", "CAM-08", "CAM-09"],
        "camera.export_settings" | "camera.import_settings" => &["CAM-07"],
        "file.save_capture" | "camera.save_capture" => &["FILE-14"],
        _ => &[],
    }
}

/// Run command `id` with JSON arguments, without a window. None when `id` is not ours.
pub fn execute(app: &mut DarkBurn, id: &str, args: &Value) -> Option<Result<Value, String>> {
    if !handles(id) {
        return None;
    }
    let null = json!({});
    let a = if args.is_null() { &null } else { args };
    sync(app);
    Some(match id {
        "laser.pnc_first" | "laser.pnc_second" => (|| {
            let idx = if id == "laser.pnc_first" { 0 } else { 1 };
            // Without a position the target is where the laser head stands now.
            let laser = if a.get("x").is_none() && a.get("y").is_none() {
                crate::motion_cmds::head_target(app, flag(a, "pointer").unwrap_or(false))?
            } else {
                point_of(a)?
            };
            let design = match (num(a, "design_x"), num(a, "design_y")) {
                (Some(x), Some(y)) => Some(Point::new(x, y)),
                _ => None,
            };
            pnc_set_target(app, idx, laser, design)
        })(),
        "pnc.set_target" => (|| {
            let idx = a.get("index").and_then(Value::as_u64).filter(|i| *i < 4).ok_or("index must be 0 to 3")? as usize;
            let design = match (num(a, "design_x"), num(a, "design_y")) {
                (Some(x), Some(y)) => Some(Point::new(x, y)),
                _ => None,
            };
            pnc_set_target(app, idx, point_of(a)?, design)
        })(),
        "laser.pnc_align" => pnc_align(app, PnCMode::Scaled),
        "laser.pnc_align_noscale" => pnc_align(app, PnCMode::Unscaled),
        "laser.pnc_reset" => Ok(pnc_reset(app)),
        "pnc.get" | "laser.print_and_cut" => Ok(pnc_json(app)),
        "pnc.wizard" | "laser.pnc_wizard" => wizard_cmd(app, a),
        "camera.get" => Ok(camera_json(app)),
        "camera.set" => camera_set(app, a),
        "camera.find" => crate::camera_devices::find(app),
        "camera.start" => crate::camera_devices::start(app, a),
        "camera.stop" => Ok(crate::camera_devices::stop(app)),
        "camera.load_frame" => path_arg(a).and_then(|p| load_frame_file(app, &p)).map(|(w, h)| json!({ "width_px": w, "height_px": h })),
        "camera.update_overlay" | "laser.capture" => update_overlay(app),
        "camera.trace" => trace_overlay(app, a),
        "camera.trace_picture" => crate::capture_trace::command(app, a),
        "camera.save_settings" => save_settings(app),
        "camera.export_settings" => path_arg(a).and_then(|p| export_settings(app, &p)),
        "camera.import_settings" => path_arg(a).and_then(|p| import_settings(app, &p)),
        "camera.save_capture" | "file.save_capture" => path_arg(a).and_then(|p| save_capture(app, &p)),
        "camera.lens_calibrate" | "laser.camera_lens" => lens_calibrate_cmd(app, a),
        "camera.lens_clear" => Ok(clear_lens(app)),
        "camera.align_generate" => align_generate(app, a),
        "camera.align_solve" | "laser.camera_alignment" => align_solve_cmd(app, a),
        "camera.align_clear" => Ok(align_clear(app)),
        "camera.head_solve" => head_solve_cmd(app, a),
        "camera.head_add_capture" => (|| {
            // The picture: a file, else the latest camera picture. The head: x and y, else where
            // the connected laser reports it.
            let img = if a.get("path").is_some() {
                let bytes = std::fs::read(path_arg(a)?).map_err(project_error)?;
                crate::images::decode(&bytes)?.to_rgba8()
            } else {
                app.vision.raw.clone().ok_or("there is no camera picture yet")?
            };
            let typed = match (num(a, "x"), num(a, "y")) {
                (Some(x), Some(y)) => Some(Point::new(x, y)),
                (None, None) => None,
                _ => return Err("give both x and y, or neither".into()),
            };
            let head = head_now(app, typed)?;
            let n = head_add_capture(app, img, head);
            Ok(json!({ "captures": n, "head": { "x": head.x, "y": head.y } }))
        })(),
        "camera.head_stitch" => head_stitch(app),
        "rotary.set" | "laser.rotary" => {
            if a.as_object().is_some_and(|o| o.is_empty()) {
                Ok(rotary_json(app))
            } else {
                rotary_set(app, a)
            }
        }
        "rotary.test_plan" => rotary_test_plan(app, a),
        "rotary.split" => rotary_split(app),
        "cylinder.set" | "laser.cylinder" => {
            if a.as_object().is_some_and(|o| o.is_empty()) {
                Ok(json!({ "cylinder": app.vision.state.cylinder }))
            } else {
                cylinder_set(app, a)
            }
        }
        "test.focus" | "laser.focus_test" => focus_generate(app, a),
        "test.interval" | "laser.interval_test" => interval_generate(app, a),
        "test.centre" | "laser.centre_finder" => (|| {
            let pts = points(a, "points")?;
            let layer = layer_arg(app, a, "layer", app.editor.current_layer)?;
            centre_find(app, &pts, flag(a, "mark").unwrap_or(false), layer)
        })(),
        "test.feeder" | "laser.feeder" => feeder_calc(app, a),
        other => Err(format!("{} is not available without a window", canonical(other))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::prefs::Prefs;
    use darkburn_vision::capture::CannedFrames;
    use darkburn_vision::printcut::PnCStep;
    use image::Rgba;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
        execute_headless(app, id, &args)
    }

    /// A synthetic camera picture: a 640x480 grey field with a dark square.
    fn frame() -> RgbaImage {
        RgbaImage::from_fn(640, 480, |x, y| {
            if (200..440).contains(&x) && (120..360).contains(&y) { Rgba([10, 10, 10, 255]) } else { Rgba([220, 220, 220, 255]) }
        })
    }

    #[test]
    fn none_of_these_count_as_hardware() {
        for id in ["laser.rotary", "laser.pnc_first", "laser.focus_test", "laser.capture", "laser.cylinder", "camera.set"] {
            assert!(!crate::commands::hardware(id), "{id}");
            assert!(crate::commands::works(id), "{id}");
        }
        assert!(crate::commands::hardware("laser.controller_info"));
    }

    #[test]
    fn every_registered_command_has_a_handler() {
        for c in COMMANDS {
            assert!(handles(c.id));
            assert!(!checklist(c.id).is_empty() || c.id == "camera.lens_clear", "{} has no checklist ids", c.id);
        }
    }

    #[test]
    fn rotary_setup_validates_saves_and_plans_a_test() {
        let mut app = app();
        let r = run(
            &mut app,
            "rotary.set",
            json!({ "enabled": true, "kind": "roller", "axis": "A", "roller_diameter": 20.0, "diameter": 60.0, "mirror": true }),
        )
        .unwrap();
        assert_eq!(r["rotary"]["kind"], "roller");
        assert!(app.editor.doc.vision_settings.is_some());
        assert!(run(&mut app, "rotary.set", json!({ "steps_per_rotation": 0 })).is_err(), "bad steps are refused");
        assert_eq!(app.vision.state.rotary.steps_per_rotation, 1600.0, "a refused change leaves the setup alone");
        let plan = run(&mut app, "rotary.test_plan", json!({ "revolutions": 1 })).unwrap();
        assert_eq!(plan["sent"], false);
        let moves = plan["plan"]["moves"].as_array().unwrap();
        assert_eq!(moves[0]["distance"].as_f64().unwrap(), -moves[1]["distance"].as_f64().unwrap());
        assert!(app.editor.history.can_undo());
        app.editor.undo();
        sync(&mut app);
        assert!(!app.vision.state.rotary.enabled, "undo turns it off again");
    }

    #[test]
    fn rotary_splits_the_design_for_galvo() {
        let mut app = app();
        run(&mut app, "shape.add_rect", json!({ "x": 0, "y": 0, "width": 40, "height": 120 })).unwrap();
        run(&mut app, "rotary.set", json!({ "enabled": true, "diameter": 30, "split": "slice", "split_max_size": 50 })).unwrap();
        let r = run(&mut app, "rotary.split", json!({})).unwrap();
        assert!(r["slices"].as_array().unwrap().len() >= 2);
        run(&mut app, "rotary.set", json!({ "split": "off" })).unwrap();
        assert!(run(&mut app, "rotary.split", json!({})).is_err());
    }

    #[test]
    fn cylinder_correction_is_stored_and_refuses_a_bad_radius() {
        let mut app = app();
        let r = run(&mut app, "cylinder.set", json!({ "enabled": true, "diameter": 80, "kind": "concave", "axis": "along_y" })).unwrap();
        assert_eq!(r["cylinder"]["params"]["radius"], 40.0);
        assert_eq!(r["cylinder"]["params"]["kind"], "concave");
        assert!(run(&mut app, "cylinder.set", json!({ "radius": -1 })).is_err());
    }

    #[test]
    fn print_and_cut_targets_align_and_reset() {
        let mut app = app();
        run(&mut app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 100, "height": 50 })).unwrap();
        assert!(run(&mut app, "laser.pnc_align", json!({})).is_err(), "no targets yet");
        // the design corners are (10,10) and (110,10); the print sits 5 mm right and 3 mm up, rotated 0
        run(&mut app, "laser.pnc_first", json!({ "x": 15, "y": 13 })).unwrap();
        run(&mut app, "laser.pnc_second", json!({ "x": 115, "y": 13 })).unwrap();
        let r = run(&mut app, "laser.pnc_align_noscale", json!({})).unwrap();
        assert!(r["rms_mm"].as_f64().unwrap() < 1e-6);
        assert!((r["translation"][0].as_f64().unwrap() - 5.0).abs() < 1e-6);
        assert!(mode_status(&app.vision.state).contains("Print and cut: on"));
        // the registration reaches the job
        let map = crate::vision_state::JobMap::from_doc(&app.editor.doc).unwrap();
        let mut p = darkburn_core::kurbo::BezPath::new();
        p.move_to((10.0, 10.0));
        p.line_to((20.0, 10.0));
        assert!((map.apply(p).bounding_box().x0 - 15.0).abs() < 1e-6);
        run(&mut app, "laser.pnc_reset", json!({})).unwrap();
        assert!(mode_status(&app.vision.state).is_empty());
        assert!(app.vision.state.pnc.fit.is_none());
    }

    /// The laser positions are what the controller reports: machine coordinates. On a machine
    /// whose origin is at the back (Ruida) Y runs the other way from the design.
    #[test]
    fn print_and_cut_on_a_machine_with_its_origin_at_the_back() {
        use darkburn_job::origin::Corner;
        for (corner, wizard) in [(Corner::TopLeft, false), (Corner::TopRight, false), (Corner::BottomLeft, false), (Corner::TopLeft, true)] {
            let mut app = app();
            app.machine.devices.active_mut().unwrap().origin = corner;
            let to_machine = app.machine.active().job_device().geometry().affine();
            run(&mut app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 100, "height": 50 })).unwrap();
            // the print sits 5 mm right and 3 mm up of the design: where the laser stands on the two marks
            let m1 = to_machine * Point::new(15.0, 13.0);
            let m2 = to_machine * Point::new(115.0, 13.0);
            let fit = if wizard {
                run(&mut app, "pnc.wizard", json!({ "start": true, "targets": [{"x":10,"y":10},{"x":110,"y":10}] })).unwrap();
                run(&mut app, "pnc.wizard", json!({ "event": "select_mode", "mode": "unscaled" })).unwrap();
                run(&mut app, "pnc.wizard", json!({ "event": "measured", "x": m1.x, "y": m1.y })).unwrap();
                run(&mut app, "pnc.wizard", json!({ "event": "measured", "x": m2.x, "y": m2.y })).unwrap();
                run(&mut app, "pnc.wizard", json!({ "event": "confirm" })).unwrap();
                app.vision.state.pnc.fit.clone().unwrap()
            } else {
                run(&mut app, "laser.pnc_first", json!({ "x": m1.x, "y": m1.y })).unwrap();
                run(&mut app, "laser.pnc_second", json!({ "x": m2.x, "y": m2.y })).unwrap();
                run(&mut app, "laser.pnc_align_noscale", json!({})).unwrap();
                // the position reads back as it was entered
                assert_eq!(app.vision.state.pnc.measured[0], Some(m1), "{corner:?}");
                app.vision.state.pnc.fit.clone().unwrap()
            };
            assert!(fit.rotation_deg.abs() < 1e-6, "{corner:?} wizard {wizard}: turned by {}", fit.rotation_deg);
            assert!((fit.translation.0 - 5.0).abs() < 1e-6 && (fit.translation.1 - 3.0).abs() < 1e-6, "{corner:?}: {:?}", fit.translation);
            // the job cuts where the print is
            let job = crate::job_build::build_job_simple(&crate::job_cmds::build_input(&app)).unwrap();
            let b = job.bounds;
            assert!((b.x0 - 15.0).abs() < 0.01 && (b.y0 - 13.0).abs() < 0.01 && (b.x1 - 115.0).abs() < 0.01, "{corner:?}: {b:?}");
        }
    }

    #[test]
    fn print_and_cut_with_scaling_finds_the_scale() {
        let mut app = app();
        run(&mut app, "laser.pnc_first", json!({ "x": 0, "y": 0, "design_x": 0, "design_y": 0 })).unwrap();
        run(&mut app, "laser.pnc_second", json!({ "x": 202, "y": 0, "design_x": 200, "design_y": 0 })).unwrap();
        let r = run(&mut app, "laser.pnc_align", json!({})).unwrap();
        assert!((r["scale"].as_f64().unwrap() - 1.01).abs() < 1e-9);
    }

    #[test]
    fn print_and_cut_wizard_walks_through_and_survives_a_reload() {
        let mut app = app();
        run(&mut app, "pnc.wizard", json!({ "start": true, "targets": [{"x":0,"y":0},{"x":100,"y":0}] })).unwrap();
        assert!(run(&mut app, "pnc.wizard", json!({ "event": "confirm" })).is_err(), "wrong step");
        run(&mut app, "pnc.wizard", json!({ "event": "select_mode", "mode": "scaled" })).unwrap();
        run(&mut app, "pnc.wizard", json!({ "event": "measured", "x": 1, "y": 1 })).unwrap();
        // save and reopen mid-way
        let bytes = darkburn_core::project::save(&app.editor.doc, "t", None).unwrap();
        let loaded = darkburn_core::project::load(&bytes).unwrap();
        let mut again = DarkBurn::new(Prefs::default());
        again.editor.doc = loaded.doc;
        sync(&mut again);
        let w = again.vision.state.pnc.wizard.clone().unwrap();
        assert_eq!(w.step, PnCStep::MeasureTarget(1));
        run(&mut again, "pnc.wizard", json!({ "event": "measured", "x": 101, "y": 1 })).unwrap();
        run(&mut again, "pnc.wizard", json!({ "event": "confirm" })).unwrap();
        assert!(again.vision.state.pnc.fit.is_some(), "a confirmed wizard activates the registration");
        run(&mut again, "pnc.wizard", json!({ "event": "reset" })).unwrap();
        assert!(again.vision.state.pnc.fit.is_none());
    }

    #[test]
    fn camera_overlay_from_a_synthetic_frame() {
        let mut app = app();
        assert!(run(&mut app, "camera.update_overlay", json!({})).is_err(), "no frame yet");
        let dir = temp("frame");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("frame.png");
        frame().save(&f).unwrap();
        let r = run(&mut app, "camera.load_frame", json!({ "path": f.display().to_string() })).unwrap();
        assert_eq!(r["width_px"], 640);
        let o = app.vision.overlay.as_ref().unwrap();
        assert_eq!(o.covers, bed(&app), "unaligned: stretched over the bed");
        let before_gen = app.vision.overlay_gen;
        run(&mut app, "camera.set", json!({ "fade": 0.5, "color": false, "x_shift": 5.0, "width_pct": 50.0 })).unwrap();
        let c = &app.vision.state.camera;
        assert_eq!(c.state.fade, 0.5);
        assert!(!c.state.color);
        assert!((c.overlay.width_mm.unwrap() - bed(&app).width / 2.0).abs() < 1e-9);
        assert!((c.overlay.placement(app.vision.overlay.as_ref().unwrap().covers).x0 - 5.0).abs() < 1e-9);
        // a black and white overlay has equal channels
        let shown = c.state.apply(&app.vision.overlay.as_ref().unwrap().img).unwrap();
        assert!(shown.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]));
        run(&mut app, "camera.update_overlay", json!({})).unwrap();
        assert!(app.vision.overlay_gen > before_gen);
        run(&mut app, "camera.set", json!({ "show": false })).unwrap();
        assert!(app.vision.state.camera.state.apply(&app.vision.overlay.as_ref().unwrap().img).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn camera_slider_changes_leave_the_project_needing_a_save() {
        let mut app = app();
        assert!(!app.editor.is_modified());
        run(&mut app, "camera.set", json!({ "fade": 0.5 })).unwrap();
        assert!(app.editor.doc.vision_settings.is_some(), "the fade is in the project");
        assert!(app.editor.is_modified(), "so the project has a change to save");
        assert!(!app.editor.history.can_undo(), "a slider is still not an undo step");
        app.editor.mark_saved();
        // the same value again changes nothing
        run(&mut app, "camera.set", json!({ "fade": 0.5 })).unwrap();
        assert!(!app.editor.is_modified());
        run(&mut app, "camera.set", json!({ "x_shift": 3.0 })).unwrap();
        assert!(app.editor.is_modified());
        app.editor.mark_saved();
        // the feeder numbers are kept the same way
        run(&mut app, "test.feeder", json!({ "total_length": 2000.0 })).unwrap();
        assert!(app.editor.is_modified());
    }

    #[test]
    fn canned_source_feeds_the_overlay() {
        let mut app = app();
        app.vision.source = Some(Box::new(CannedFrames::new([frame()])));
        run(&mut app, "camera.update_overlay", json!({})).unwrap();
        assert!(app.vision.overlay.is_some());
        assert!(run(&mut app, "camera.update_overlay", json!({})).is_err(), "the canned frames ran out");
    }

    #[test]
    fn camera_alignment_places_the_overlay_on_the_bed_and_traces_at_true_size() {
        let mut app = app();
        app.prefs.page_width = 200.0;
        app.prefs.page_height = 100.0;
        // the camera sees the bed exactly: 640x480 pixels over 200x100 mm, so 0.3125 mm/px in x
        let g = run(&mut app, "camera.align_generate", json!({ "margin": 10, "count": 4 })).unwrap();
        assert_eq!(g["created"].as_array().unwrap().len(), 4);
        let t = g["targets"].as_array().unwrap();
        assert_eq!(t.len(), 4);
        // where those targets appear in the picture: (10,10) is bottom-left => pixel (32, 432) etc
        let px = |x: f64, y: f64| json!({ "x": x / 200.0 * 640.0, "y": (100.0 - y) / 100.0 * 480.0 });
        let clicks: Vec<Value> = t.iter().map(|p| px(p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap())).collect();
        let f = temp("align").join("f.png");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        frame().save(&f).unwrap();
        run(&mut app, "camera.load_frame", json!({ "path": f.display().to_string() })).unwrap();
        let r = run(&mut app, "camera.align_solve", json!({ "points": clicks })).unwrap();
        assert!(r["rms_mm"].as_f64().unwrap() < 1e-6, "{r}");
        let o = app.vision.overlay.as_ref().unwrap();
        assert!((o.covers.width - 200.0).abs() < 1.0 && (o.covers.height - 100.0).abs() < 1.0, "{:?}", o.covers);
        // the dark square spans pixels 200..440 x 120..360 => mm 62.5..137.5 wide
        let r = run(&mut app, "camera.trace", json!({})).unwrap();
        let id = ShapeId(r["created"][0].as_u64().unwrap());
        let bb = app.editor.doc.get(id).unwrap().bbox();
        assert!((bb.width() - 75.0).abs() < 3.0, "traced width {}", bb.width());
        assert!((bb.x0 - 62.5).abs() < 3.0, "traced left {}", bb.x0);
        // survives save and load
        let bytes = darkburn_core::project::save(&app.editor.doc, "t", None).unwrap();
        let back = darkburn_core::project::load(&bytes).unwrap();
        assert_eq!(VisionState::from_doc(&back.doc).camera.alignment, app.vision.state.camera.alignment);
        run(&mut app, "camera.align_clear", json!({})).unwrap();
        assert!(app.vision.state.camera.alignment.is_none());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn alignment_needs_enough_targets() {
        let mut app = app();
        let e = run(&mut app, "camera.align_solve", json!({ "points": [{"x":1,"y":1}], "image_width": 640, "image_height": 480 })).unwrap_err();
        assert!(e.contains("3 targets"), "{e}");
    }

    #[test]
    fn head_camera_calibration_and_stitching() {
        let mut app = app();
        // a head camera whose picture shows 40x30 mm around the head at 8 px/mm, 320x240 px
        let marks: Vec<Value> = [(40.0, 30.0), (280.0, 30.0), (280.0, 210.0), (40.0, 210.0), (160.0, 120.0)]
            .iter()
            .map(|(px, py)| {
                let (hx, hy) = (100.0, 100.0);
                // image x right = bed x right; image y down = bed y down
                let bed = json!({ "x": hx + (px - 160.0) / 8.0, "y": hy - (py - 120.0) / 8.0 });
                json!({ "image": {"x": px, "y": py}, "head": {"x": hx, "y": hy}, "bed": bed })
            })
            .collect();
        let r = run(&mut app, "camera.head_solve", json!({ "marks": marks, "image_width": 320, "image_height": 240 })).unwrap();
        assert!(r["rms_mm"].as_f64().unwrap() < 1e-6);
        assert_eq!(app.vision.state.camera.kind, CameraKind::HeadMounted);
        assert!(head_stitch(&mut app).is_err(), "no captures yet");
        head_add_capture(&mut app, RgbaImage::from_pixel(320, 240, Rgba([200, 50, 50, 255])), Point::new(100.0, 100.0));
        let r = head_stitch(&mut app).unwrap();
        assert_eq!(r["captures"], 1);
        let o = app.vision.overlay.as_ref().unwrap();
        let ppm = o.img.width() as f64 / o.covers.width;
        // the centre of the capture (the head position) is coloured, the far corner is not covered
        let c = o.img.get_pixel((100.0 * ppm) as u32, ((o.covers.height - 100.0) * ppm) as u32);
        assert_eq!(c.0[3], 255);
        assert_eq!(o.img.get_pixel(1, 1).0[3], 0);
    }

    /// The head position is what the controller reports. On a machine with its origin at the back
    /// right that is not the design position: the stitched picture still lands where the head was
    /// over the design, not mirrored to the other side of the bed.
    #[test]
    fn head_camera_captures_land_under_the_head_on_a_machine_with_its_origin_at_the_back() {
        let mut app = app();
        app.machine.devices.active_mut().unwrap().origin = darkburn_job::origin::Corner::TopRight;
        let b = bed(&app);
        // The head stands over the design point (60, 50); the controller reports it from the far corner.
        let design_head = Point::new(60.0, 50.0);
        let reported = machine_affine(&app) * design_head;
        assert!((reported - design_head).hypot() > 1.0, "this machine counts from another corner");
        // 8 px/mm, picture x right = design x right, picture y down = design y down
        let marks: Vec<HeadMark> = [(40.0, 30.0), (280.0, 30.0), (280.0, 210.0), (40.0, 210.0), (160.0, 120.0)]
            .iter()
            .map(|&(px, py)| HeadMark {
                image: Point::new(px, py),
                head_pos: reported,
                bed: Point::new(design_head.x + (px - 160.0) / 8.0, design_head.y - (py - 120.0) / 8.0),
            })
            .collect();
        let r = head_solve(&mut app, marks, (320, 240)).unwrap();
        assert!(r["rms_mm"].as_f64().unwrap() < 1e-6);
        // A capture with the head somewhere else: 40 mm to the right in the design.
        let second = Point::new(100.0, 50.0);
        let reported = machine_affine(&app) * second;
        head_add_capture(&mut app, RgbaImage::from_pixel(320, 240, Rgba([200, 50, 50, 255])), reported);
        head_stitch(&mut app).unwrap();
        let o = app.vision.overlay.as_ref().unwrap();
        let ppm = o.img.width() as f64 / o.covers.width;
        let at = |p: Point| o.img.get_pixel((p.x * ppm) as u32, ((o.covers.height - p.y) * ppm) as u32).0[3];
        assert_eq!(at(second), 255, "the capture is not where the head was over the design");
        let mirrored = Point::new(b.width - second.x, b.height - second.y);
        assert_eq!(at(mirrored), 0, "the capture landed mirrored");
    }

    /// The head camera wizard shows the lens-corrected picture; the clicks made in it must not be
    /// lens-corrected a second time.
    #[test]
    fn head_camera_marks_clicked_in_the_corrected_picture() {
        let mut app = app();
        let mut lens = darkburn_vision::lens::LensModel::ideal(640, 480, 500.0);
        lens.k[0] = -0.25;
        sync(&mut app);
        app.vision.state.camera.lens = Some(lens.clone());
        commit(&mut app, "Lens", true);
        app.vision.raw = Some(frame());
        // corrected pixel to millimetres from the head: 0.1 mm per pixel about the picture centre, Y up
        let offset = |u: Point| Point::new((u.x - 319.5) * 0.1, (239.5 - u.y) * 0.1);
        let head = Point::new(100.0, 80.0);
        let clicks = [Point::new(60.0, 50.0), Point::new(580.0, 60.0), Point::new(570.0, 430.0), Point::new(70.0, 420.0), Point::new(320.0, 240.0)];
        let marks: Vec<HeadMark> =
            clicks.iter().map(|u| head_mark(&app, *u, head, Point::new(head.x + offset(*u).x, head.y + offset(*u).y))).collect();
        for (m, u) in marks.iter().zip(&clicks) {
            assert!((mark_in_view(&app, m) - *u).hypot() < 1e-6, "the mark is drawn where it was clicked");
        }
        let r = head_solve(&mut app, marks, (640, 480)).unwrap();
        assert!(r["rms_mm"].as_f64().unwrap() < 1e-6, "{r}");
        // a spot seen at corrected pixel (200, 150) is at this pixel of the camera's own picture
        let u = Point::new(200.0, 150.0);
        let raw = lens.distort_point_with(u, 1.0);
        assert!((raw - u).hypot() > 2.0, "the lens bends this spot noticeably");
        let cal = app.vision.state.camera.head.clone().unwrap();
        let got = cal.bed_point(head, raw).unwrap();
        let want = Point::new(head.x + offset(u).x, head.y + offset(u).y);
        assert!((got - want).hypot() < 1e-4, "{got:?} against {want:?}");
    }

    /// An alignment is clicked in the lens-corrected picture: when that correction changes (the
    /// focal scale, no lens any more) the alignment is dropped, not kept to place the overlay wrongly.
    #[test]
    fn an_alignment_does_not_outlive_the_lens_correction_it_was_made_with() {
        let clicks = json!([{ "x": 32, "y": 432 }, { "x": 608, "y": 432 }, { "x": 608, "y": 48 }, { "x": 32, "y": 48 }]);
        let bed = json!([{ "x": 10, "y": 10 }, { "x": 190, "y": 10 }, { "x": 190, "y": 90 }, { "x": 10, "y": 90 }]);
        let aligned = |with_lens: bool| {
            let mut app = app();
            sync(&mut app);
            if with_lens {
                let mut lens = darkburn_vision::lens::LensModel::ideal(640, 480, 500.0);
                lens.k[0] = -0.25;
                app.vision.state.camera.lens = Some(lens);
                commit(&mut app, "Lens", true);
            }
            run(&mut app, "camera.align_solve", json!({ "points": clicks, "bed": bed, "image_width": 640, "image_height": 480 })).unwrap();
            assert!(app.vision.state.camera.alignment.is_some());
            app.vision.message = None;
            app
        };
        // Another focal scale: the corrected picture is another picture.
        let mut app = aligned(true);
        run(&mut app, "camera.set", json!({ "focal_scale": 1.0, "fade": 0.5 })).unwrap();
        assert!(app.vision.state.camera.alignment.is_some(), "the same focal scale and other settings change nothing");
        run(&mut app, "camera.set", json!({ "focal_scale": 1.3 })).unwrap();
        assert!(app.vision.state.camera.alignment.is_none(), "the alignment was kept though the focal scale changed");
        assert!(app.vision.message.as_deref().is_some_and(|m| m.contains("alignment again")), "{:?}", app.vision.message);
        assert_eq!(run(&mut app, "camera.get", json!({})).unwrap()["aligned"], false);
        // The lens calibration is cleared.
        let mut app = aligned(true);
        run(&mut app, "camera.lens_clear", json!({})).unwrap();
        assert!(app.vision.state.camera.alignment.is_none(), "the alignment was kept though the lens calibration was cleared");
        // One undo brings back the lens and the alignment made with it.
        app.editor.undo();
        sync(&mut app);
        assert!(app.vision.state.camera.lens.is_some() && app.vision.state.camera.alignment.is_some());
        // Without a lens the focal scale corrects nothing, and clearing no lens changes nothing.
        let mut app = aligned(false);
        run(&mut app, "camera.set", json!({ "focal_scale": 1.3 })).unwrap();
        run(&mut app, "camera.lens_clear", json!({})).unwrap();
        assert!(app.vision.state.camera.alignment.is_some());
        assert_eq!(app.vision.message, None);
    }

    #[test]
    fn settings_export_import_and_remember_last_camera() {
        let mut app = app();
        let dir = temp("settings");
        std::fs::create_dir_all(&dir).unwrap();
        app.settings_dir = Some(dir.clone());
        run(&mut app, "camera.set", json!({ "camera_name": "Desk cam", "device_index": 2, "exposure": -5.0 })).unwrap();
        let lens = darkburn_vision::lens::LensModel {
            name: "wide".into(),
            width: 640,
            height: 480,
            fx: 500.0,
            fy: 501.0,
            cx: 320.0,
            cy: 240.0,
            k: [0.1, -0.02, 0.0, 0.0],
            kind: darkburn_vision::lens::LensKind::Fisheye,
        };
        app.vision.state.camera.lens = Some(lens.clone());
        run(&mut app, "camera.save_settings", json!({})).unwrap();
        assert!(dir.join("camera.json").exists());
        let f = dir.join("export.json");
        run(&mut app, "camera.export_settings", json!({ "path": f.display().to_string() })).unwrap();
        // a fresh app remembers the last camera
        let mut fresh = DarkBurn::new(Prefs::default());
        fresh.settings_dir = Some(dir.clone());
        sync(&mut fresh);
        assert_eq!(fresh.vision.state.camera.camera_name, "Desk cam");
        assert_eq!(fresh.vision.prefs.last_device_index, Some(2));
        // import into another
        let mut other = DarkBurn::new(Prefs::default());
        run(&mut other, "camera.import_settings", json!({ "path": f.display().to_string() })).unwrap();
        assert_eq!(other.vision.state.camera.exposure, Some(-5.0));
        assert_eq!(other.vision.state.camera.lens, Some(lens), "the lens calibration travels in the file");
        assert!(run(&mut other, "camera.import_settings", json!({ "path": dir.join("nothing.json").display().to_string() })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_capture_writes_a_picture() {
        let mut app = app();
        assert!(run(&mut app, "file.save_capture", json!({ "path": "x.png" })).is_err());
        set_frame(&mut app, frame()).unwrap();
        let dir = temp("cap");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("cap.png");
        run(&mut app, "file.save_capture", json!({ "path": f.display().to_string() })).unwrap();
        assert!(image::open(&f).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_becomes_a_selected_picture_for_the_trace_window() {
        let mut app = app();
        set_frame(&mut app, frame()).unwrap();
        let p = overlay_picture(&app).unwrap();
        let id = add_overlay_picture(&mut app, p);
        assert!(matches!(app.editor.doc.get(id).unwrap().kind, ShapeKind::Image(_)));
        assert_eq!(app.editor.selected(), vec![id]);
        assert!(crate::image_ui::TraceDialog::new(&app, vec![id]).is_ok());
    }

    #[test]
    fn lens_wizard_refuses_pictures_without_the_pattern() {
        let mut app = app();
        set_frame(&mut app, frame()).unwrap();
        let spec = pattern_from(&json!({})).unwrap();
        let e = calib_add_frame(&mut app, &spec).unwrap_err();
        assert!(e.contains("not found"), "{e}");
        assert!(run(&mut app, "camera.lens_calibrate", json!({})).is_err(), "no pictures");
        assert!(pattern_from(&json!({ "pattern": { "cols": 1 } })).is_err());
        assert!(pattern_from(&json!({ "pattern": { "kind": "stars" } })).is_err());
    }

    #[test]
    fn lens_calibration_from_synthetic_views() {
        // A flat circle grid photographed from slightly different spots: not enough variety to
        // pin a lens down, but the pictures must be found and the wizard must say so cleanly.
        let mut app = app();
        let spec = PatternSpec { kind: PatternKind::CircleGrid { dark_on_light: true }, cols: 5, rows: 4, spacing_mm: 20.0 };
        for shift in [0.0, 12.0, 24.0] {
            let img = RgbaImage::from_fn(640, 480, |x, y| {
                let (fx, fy) = (x as f64 - 120.0 - shift, y as f64 - 120.0);
                let (cx, cy) = ((fx / 60.0).round(), (fy / 60.0).round());
                let inside = (0.0..5.0).contains(&cx) && (0.0..4.0).contains(&cy) && (fx - cx * 60.0).hypot(fy - cy * 60.0) < 16.0;
                if inside { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
            });
            app.vision.raw = Some(img);
            calib_add_frame(&mut app, &spec).unwrap();
        }
        assert_eq!(app.vision.calib_frames.len(), 3);
        let r = calibrate_lens(&mut app, &spec, false, 2);
        // Either a model comes out or the solver says why not; it never panics or hangs.
        match r {
            Ok(v) => assert!(v["rms_px"].as_f64().is_some()),
            Err(e) => assert!(!e.is_empty()),
        }
    }

    #[test]
    fn focus_and_interval_tests_land_in_the_document() {
        let mut app = app();
        let r = run(&mut app, "laser.focus_test", json!({ "steps": 5, "z_start": -1, "z_end": 1, "x": 5, "y": 5 })).unwrap();
        assert_eq!(r["layers"].as_array().unwrap().len(), 6);
        assert_eq!(app.editor.doc.layer(0).main().z_offset, -1.0);
        assert_eq!(app.vision.state.focus.steps, 5, "the settings are kept");
        let r = run(&mut app, "laser.interval_test", json!({ "steps": 4, "first_layer": 10, "interval_start": 0.05, "interval_end": 0.2 })).unwrap();
        assert_eq!(r["layers"][0], 10);
        assert!((app.editor.doc.layer(11).main().interval - 0.1).abs() < 1e-9);
        // one undo removes the whole interval test
        let before = app.editor.doc.iter().count();
        app.editor.undo();
        assert!(app.editor.doc.iter().count() < before);
        // errors do not leave half a test behind
        let n = app.editor.doc.iter().count();
        assert!(run(&mut app, "test.focus", json!({ "steps": 1 })).is_err());
        assert!(run(&mut app, "test.focus", json!({ "first_layer": 99 })).is_err());
        assert_eq!(app.editor.doc.iter().count(), n);
    }

    #[test]
    fn centre_finder_and_feeder_calculations() {
        let mut app = app();
        let pts = [(60.0, 50.0), (50.0, 60.0), (40.0, 50.0), (50.0, 40.0)].map(|(x, y)| json!({"x": x, "y": y}));
        let r = run(&mut app, "laser.centre_finder", json!({ "points": pts, "mark": true })).unwrap();
        assert!((r["diameter"].as_f64().unwrap() - 20.0).abs() < 1e-6);
        assert!((r["centre"]["x"].as_f64().unwrap() - 50.0).abs() < 1e-6);
        assert_eq!(r["created"].as_array().unwrap().len(), 1);
        assert!(run(&mut app, "test.centre", json!({ "points": [{"x":0,"y":0},{"x":1,"y":1},{"x":2,"y":2}] })).is_err(), "collinear");
        let f = run(&mut app, "laser.feeder", json!({ "total_length": 1000, "window_height": 300, "overlap": 10, "current_steps_per_mm": 80, "commanded": 100, "measured": 98, "draw": true, "layer": 7 })).unwrap();
        assert_eq!(f["windows"], 4);
        assert!((f["corrected_steps_per_mm"].as_f64().unwrap() - 81.632653).abs() < 1e-5);
        assert_eq!(f["drawn"]["created"].as_array().unwrap().len(), 8);
        assert!(run(&mut app, "test.feeder", json!({ "overlap": 400 })).is_err());
    }

    /// The edge points are laser positions (machine coordinates); the mark goes in the design.
    #[test]
    fn centre_mark_lands_under_the_laser_on_a_machine_with_its_origin_at_the_back() {
        let mut app = app();
        app.machine.devices.active_mut().unwrap().origin = darkburn_job::origin::Corner::TopLeft;
        let height = app.machine.active().job_device().bed_h;
        let pts = [(60.0, 30.0), (50.0, 40.0), (40.0, 30.0), (50.0, 20.0)].map(|(x, y)| json!({"x": x, "y": y}));
        let r = run(&mut app, "laser.centre_finder", json!({ "points": pts, "mark": true })).unwrap();
        assert!((r["centre"]["y"].as_f64().unwrap() - 30.0).abs() < 1e-6, "reported as a laser position");
        let id = ShapeId(r["created"][0].as_u64().unwrap());
        let c = app.editor.doc.get(id).unwrap().bbox().center();
        assert!((c.x - 50.0).abs() < 0.01 && (c.y - (height - 30.0)).abs() < 0.01, "mark at {c:?}, bed {height} high");
    }

    #[test]
    fn the_job_is_mapped_by_rotary_and_cylinder() {
        let mut app = app();
        run(&mut app, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20 })).unwrap();
        let plain = {
            let input = crate::job_cmds::build_input(&app);
            crate::job_build::build_job_simple(&input).unwrap()
        };
        run(&mut app, "rotary.set", json!({ "enabled": true, "kind": "chuck", "diameter": 100, "steps_per_rotation": 3200, "axis_steps_per_mm": 1 }))
            .unwrap();
        let rotary = {
            let input = crate::job_cmds::build_input(&app);
            crate::job_build::build_job_simple(&input).unwrap()
        };
        assert_ne!(plain.bounds, rotary.bounds, "rotary changes the job");
    }

    fn temp(name: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        std::env::temp_dir().join(format!("darkburn_vision_{name}_{n}"))
    }
}
