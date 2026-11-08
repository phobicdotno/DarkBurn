//! The bridge between the document's images and the `darkburn-raster` engine.
//!
//! # For the job builder
//!
//! ```ignore
//! images::scan_plans_for_layer(doc: &Document, layer: usize) -> Vec<darkburn_raster::ScanPlan>
//! ```
//!
//! Returns the scanlines for every image shape (groups included) on `layer`, ready for the
//! motion engine, in machine millimetres (`ScanPlan::point_mm` gives points). It is empty when
//! the layer's main cut setting is not in Image mode, or when it has no images. Everything the
//! Image layer fields say is applied: image mode (threshold with ramp, ordered, Atkinson,
//! dither, Stucki, Jarvis, newsprint, halftone cells and angle, sketch, grayscale, pass-through),
//! negative, line interval, scan angle, bi-directional, overscan, dot width correction, min and
//! max power (scaled by the shape's power scale), merging of the layer's images into one scan,
//! the image adjustments, crops and transparency. The device's fast blank-skip and scanning
//! offset table (`scan_offsets`) are applied on top.
//!
//! Related entry points, same crate:
//! * [`scan_plans_for_layer_pass`]: the plans for pass `n` (the scan angle grows by the layer's
//!   "angle increase per pass" each pass).
//! * [`try_scan_plans`]: the same with the reason when an image is too large to process.
//!   [`scan_plans_for_layer`] logs that reason and leaves the image out.
//!
//! The functions are pure reads of the document (plus the process-wide scan tuning) and may be
//! called from any thread. Decoded images are cached, so calling once per pass is cheap.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use darkburn_core::kurbo::Affine;
use darkburn_core::layer::ImageMode as LayerMode;
use darkburn_core::{CutSetting, Document, ImageAdjust, ImageData, ImageShape, Mode, Shape, ShapeKind};
use darkburn_raster::{Adjustments, GrayF, ImageMode, ImageSettings, ProcessedImage, ScanPlan, ScanSettings, apply_mask, process_image, scan_plans};
use image::DynamicImage;

use crate::scan_offsets::{self, ScanTuning};

/// Sketch mode detail size and strength (grid pixels, gain); the layer has no field for them yet.
pub const SKETCH_RADIUS_PX: f32 = 2.0;
pub const SKETCH_GAIN: f32 = 6.0;
/// Level at which Threshold mode turns on.
pub const THRESHOLD_LEVEL: f32 = 0.5;
/// Working memory per dot while an image is processed, bytes (grey, resampled copy, power map).
pub const BYTES_PER_DOT: u64 = 14;
/// Above this much working memory the Adjust Image dialog warns.
pub const MEMORY_WARN_BYTES: u64 = 1 << 30;
/// Above this many dots an image is refused.
pub const MAX_DOTS: u64 = 400_000_000;
/// Import refuses images with a side longer than this, or more pixels than [`MAX_IMPORT_PIXELS`].
pub const MAX_IMPORT_SIDE: u32 = 100_000;
pub const MAX_IMPORT_PIXELS: u64 = 250_000_000;
/// Images with more pixels than this are shrunk to this many before they are processed.
pub const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// Why an image could not be processed.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageError {
    /// The pixels are missing from the project or do not decode.
    Unreadable(String),
    /// The image would be more than [`MAX_DOTS`] dots at the layer's resolution.
    TooLarge { dots: u64, shape: u64 },
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Unreadable(why) => write!(f, "image cannot be read: {why}"),
            ImageError::TooLarge { dots, .. } => write!(
                f,
                "image would be {:.0} million dots at this resolution (the limit is {:.0} million): lower the DPI",
                *dots as f64 / 1e6,
                MAX_DOTS as f64 / 1e6
            ),
        }
    }
}

/// Checks an image about to be imported: its pixel size against the limits.
pub fn check_import_size(width: u32, height: u32) -> Result<(), String> {
    if width > MAX_IMPORT_SIDE || height > MAX_IMPORT_SIDE || u64::from(width) * u64::from(height) > MAX_IMPORT_PIXELS {
        return Err(format!(
            "the image is {width} x {height} pixels, which is too large (the limit is {} million pixels)",
            MAX_IMPORT_PIXELS / 1_000_000
        ));
    }
    Ok(())
}

/// Decodes encoded image bytes, refusing absurd sizes before allocating.
pub fn decode(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMPORT_SIDE);
    limits.max_image_height = Some(MAX_IMPORT_SIDE);
    limits.max_alloc = Some(2 << 30);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?;
    if u64::from(img.width()) * u64::from(img.height()) > MAX_SOURCE_PIXELS {
        let k = (MAX_SOURCE_PIXELS as f64 / (f64::from(img.width()) * f64::from(img.height()))).sqrt();
        let (w, h) = (((f64::from(img.width()) * k) as u32).max(1), ((f64::from(img.height()) * k) as u32).max(1));
        return Ok(img.resize_exact(w, h, image::imageops::FilterType::Triangle));
    }
    Ok(img)
}

// ----- Settings -----

/// The engine adjustments for an image's [`ImageAdjust`]. Amount is a percentage, the radius
/// defaults to one pixel when sharpening or denoising is on without one, denoise is 0 to 100.
pub fn adjustments(a: &ImageAdjust) -> Adjustments {
    let enhancing = a.enhance_amount > 0.0 || a.enhance_denoise > 0.0;
    Adjustments {
        brightness: a.brightness.clamp(-100.0, 100.0) as f32,
        contrast: a.contrast.clamp(-100.0, 100.0) as f32,
        gamma: a.gamma.clamp(0.1, 10.0) as f32,
        enhance_amount: (a.enhance_amount.max(0.0) / 100.0) as f32,
        enhance_radius: if enhancing { (a.enhance_radius as f32).max(if a.enhance_radius > 0.0 { 0.1 } else { 1.0 }) } else { 0.0 },
        enhance_denoise: (a.enhance_denoise / 100.0).clamp(0.0, 1.0) as f32,
        negative: false,
    }
}

/// The engine mode a cut setting asks for.
pub fn engine_mode(c: &CutSetting) -> ImageMode {
    if c.pass_through {
        return ImageMode::PassThrough;
    }
    match c.image_mode {
        LayerMode::Threshold => ImageMode::Threshold { threshold: THRESHOLD_LEVEL, ramp_length_mm: c.threshold_ramp.max(0.0) },
        LayerMode::Ordered => ImageMode::Ordered,
        LayerMode::Atkinson => ImageMode::Atkinson,
        LayerMode::Dither => ImageMode::FloydSteinberg,
        LayerMode::Stucki => ImageMode::Stucki,
        LayerMode::Jarvis => ImageMode::Jarvis,
        LayerMode::Newsprint => ImageMode::Newsprint,
        LayerMode::Halftone => ImageMode::Halftone { cells_per_inch: c.halftone_cells.max(1.0), angle_deg: c.halftone_angle },
        LayerMode::Sketch => ImageMode::Sketch { radius_px: SKETCH_RADIUS_PX, gain: SKETCH_GAIN },
        LayerMode::Grayscale => ImageMode::Grayscale,
    }
}

/// The engine settings for a layer's image fields, at scan angle `angle_deg`.
pub fn image_settings(c: &CutSetting, angle_deg: f64) -> ImageSettings {
    ImageSettings {
        mode: engine_mode(c),
        line_interval_mm: c.interval.max(0.001),
        scan_angle_deg: angle_deg,
        negative: c.negative,
        linear_light: true,
    }
}

/// The scan settings for a layer's fields, with the device tuning and a power scale in percent.
pub fn scan_settings(c: &CutSetting, tuning: &ScanTuning, power_scale: f64) -> ScanSettings {
    let k = (power_scale / 100.0).max(0.0) as f32;
    ScanSettings {
        bidirectional: c.bidirectional,
        overscan_mm: c.overscan_mm().max(0.0),
        blank_skip_mm: tuning.blank_skip_mm(),
        dot_width_mm: c.dot_width_correction.max(0.0),
        min_power: (c.min_power as f32 * k).clamp(0.0, 100.0),
        max_power: (c.max_power as f32 * k).clamp(0.0, 100.0),
    }
}

// ----- Geometry -----

/// Image pixels to the shape's local millimetres (centred on the origin, Y up).
pub fn local_pixel_affine(img: &ImageShape, px: (u32, u32)) -> Affine {
    let (pw, ph) = (f64::from(px.0.max(1)), f64::from(px.1.max(1)));
    Affine::new([img.width / pw, 0.0, 0.0, -img.height / ph, -img.width / 2.0, img.height / 2.0])
}

/// Image pixels to document millimetres for an image shape.
pub fn pixel_to_doc(leaf: &Shape, img: &ImageShape, px: (u32, u32)) -> Affine {
    leaf.transform * local_pixel_affine(img, px)
}

/// The size of an image shape in document millimetres (its transform's scale applied).
pub fn size_mm(leaf: &Shape, img: &ImageShape) -> (f64, f64) {
    let c = leaf.transform.as_coeffs();
    (img.width * c[0].hypot(c[1]), img.height * c[2].hypot(c[3]))
}

/// Pixel size of the image `img` uses.
pub fn pixel_size(doc: &Document, img: &ImageShape) -> Option<(u32, u32)> {
    doc.images.get(&img.image).map(|d| (d.px_width, d.px_height))
}

/// The image shapes (groups opened, document transforms applied) on `layer`.
pub fn image_leaves(doc: &Document, layer: usize) -> Vec<Shape> {
    doc.iter().flat_map(Shape::leaves).filter(|s| s.layer == layer && s.is_image()).collect()
}

// ----- Decoded images -----

type GrayKey = (String, u64);

fn crop_hash(img: &ImageShape) -> u64 {
    let mut h = DefaultHasher::new();
    if img.crop.is_some() {
        format!("{:?}{}{}", img.crop, img.width, img.height).hash(&mut h);
    }
    h.finish()
}

fn gray_cache() -> &'static Mutex<Vec<(GrayKey, Arc<GrayF>)>> {
    static CACHE: Mutex<Vec<(GrayKey, Arc<GrayF>)>> = Mutex::new(Vec::new());
    &CACHE
}

/// The grey version of an image shape's pixels, cropped by its crop path (outside the crop is
/// white, like transparency). Cached by image and crop; the last few are kept.
pub fn source_gray(doc: &Document, img: &ImageShape) -> Result<Arc<GrayF>, ImageError> {
    let data = doc.images.get(&img.image).ok_or_else(|| ImageError::Unreadable("its pixels are not in the project".into()))?;
    source_gray_of(data, img)
}

/// [`source_gray`] for pixels already in hand (so a worker thread can do it).
pub fn source_gray_of(data: &ImageData, img: &ImageShape) -> Result<Arc<GrayF>, ImageError> {
    let key: GrayKey = (img.image.clone(), crop_hash(img));
    if let Ok(c) = gray_cache().lock()
        && let Some((_, g)) = c.iter().find(|(k, _)| *k == key)
    {
        return Ok(g.clone());
    }
    let decoded = decode(&data.bytes).map_err(ImageError::Unreadable)?;
    let gray = match &img.crop {
        Some(crop) => {
            let rgba = decoded.to_rgba8();
            let masked = apply_mask(&rgba, local_pixel_affine(img, (rgba.width(), rgba.height())), std::slice::from_ref(crop), false);
            GrayF::from_dynamic(&DynamicImage::ImageRgba8(masked))
        }
        None => GrayF::from_dynamic(&decoded),
    };
    let gray = Arc::new(gray);
    if let Ok(mut c) = gray_cache().lock() {
        let bytes = |g: &GrayF| g.data.len() * 4;
        c.push((key, gray.clone()));
        while c.len() > 6 || (c.len() > 1 && c.iter().map(|(_, g)| bytes(g)).sum::<usize>() > 512 << 20) {
            c.remove(0);
        }
    }
    Ok(gray)
}

/// Forgets decoded images (after the project changed or closed).
pub fn clear_cache() {
    if let Ok(mut c) = gray_cache().lock() {
        c.clear();
    }
}

// ----- Memory -----

/// How many dots an image of `w_mm` by `h_mm` is at line interval `interval_mm`.
pub fn dots_for(w_mm: f64, h_mm: f64, interval_mm: f64) -> u64 {
    let i = interval_mm.max(1e-6);
    ((w_mm / i).ceil().max(1.0) * (h_mm / i).ceil().max(1.0)).min(1e18) as u64
}

/// Working memory needed to process `dots` dots, bytes.
pub fn memory_for(dots: u64) -> u64 {
    dots.saturating_mul(BYTES_PER_DOT)
}

/// A warning for the Adjust Image dialog when the image would need a lot of memory.
pub fn memory_warning(dots: u64) -> Option<String> {
    let bytes = memory_for(dots);
    if dots > MAX_DOTS {
        Some(format!("{:.0} million dots is more than DarkBurn can scan. Lower the DPI.", dots as f64 / 1e6))
    } else if bytes > MEMORY_WARN_BYTES {
        Some(format!("This needs about {:.1} GB of memory at this DPI. Lower the DPI if the computer slows down.", bytes as f64 / 1e9))
    } else {
        None
    }
}

// ----- Scan plans -----

/// See the module documentation.
pub fn scan_plans_for_layer(doc: &Document, layer: usize) -> Vec<ScanPlan> {
    scan_plans_for_layer_pass(doc, layer, 0)
}

/// The plans for pass `pass` (0 is the first).
pub fn scan_plans_for_layer_pass(doc: &Document, layer: usize, pass: u32) -> Vec<ScanPlan> {
    match try_scan_plans(doc, layer, pass, &scan_offsets::current()) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("layer {layer}: {e}");
            Vec::new()
        }
    }
}

/// [`scan_plans_for_layer_pass`] with an explicit device tuning, failing when an image cannot
/// be processed.
pub fn try_scan_plans(doc: &Document, layer: usize, pass: u32, tuning: &ScanTuning) -> Result<Vec<ScanPlan>, ImageError> {
    let setting = doc.layer(layer).main();
    if setting.mode != Mode::Image {
        return Ok(Vec::new());
    }
    let angle = setting.scan_angle + f64::from(pass) * setting.angle_increment;
    let settings = image_settings(setting, angle);
    // (power scale in percent, processed image): images with the same power scale merge.
    let mut processed: Vec<(f64, ProcessedImage)> = Vec::new();
    for leaf in image_leaves(doc, layer) {
        let ShapeKind::Image(img) = &leaf.kind else { continue };
        let (w, h) = size_mm(&leaf, img);
        let bb = leaf.bbox();
        let dots = dots_for(bb.width().max(w), bb.height().max(h), settings.line_interval_mm);
        if dots > MAX_DOTS {
            return Err(ImageError::TooLarge { dots, shape: leaf.id.0 });
        }
        let gray = source_gray(doc, img)?;
        let to_mm = pixel_to_doc(&leaf, img, (gray.width, gray.height));
        processed.push((leaf.power_scale, process_image(&gray, to_mm, &adjustments(&img.adjust), &settings)));
    }
    let mut plans = Vec::new();
    let mut scales: Vec<f64> = processed.iter().map(|(s, _)| *s).collect();
    scales.sort_by(f64::total_cmp);
    scales.dedup();
    for scale in scales {
        let group: Vec<&ProcessedImage> = processed.iter().filter(|(s, _)| *s == scale).map(|(_, p)| p).collect();
        let ss = scan_settings(setting, tuning, scale);
        if setting.merge_images {
            plans.extend(scan_plans(&group, &ss));
        } else {
            for p in group {
                plans.extend(scan_plans(&[p], &ss));
            }
        }
    }
    scan_offsets::shift_plans(&mut plans, setting.speed, &tuning.offsets);
    Ok(plans)
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use darkburn_core::ImageData;

    /// A PNG of `w` by `h` pixels with grey level `f(x, y)`.
    pub fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([f(x, y)]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    /// Puts a PNG into `doc` and an image shape of `w_mm` by `h_mm` centred at (cx, cy) on `layer`.
    pub fn add_image(
        doc: &mut Document,
        bytes: Vec<u8>,
        px: (u32, u32),
        size: (f64, f64),
        centre: (f64, f64),
        layer: usize,
    ) -> darkburn_core::ShapeId {
        let key = doc.add_image(ImageData { format: "png".into(), bytes: Arc::from(bytes), px_width: px.0, px_height: px.1 });
        let shape = Shape::new(
            ShapeKind::Image(ImageShape { image: key, width: size.0, height: size.1, ..Default::default() }),
            Affine::translate(darkburn_core::kurbo::Vec2::new(centre.0, centre.1)),
            layer,
        );
        doc.add(shape)
    }

    /// Layer `layer` in Image mode with the given interval.
    pub fn image_layer(doc: &mut Document, layer: usize, interval: f64) {
        let c = doc.layer_mut(layer).main_mut();
        c.mode = Mode::Image;
        c.interval = interval;
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use darkburn_core::kurbo::Shape as _;

    fn black_doc() -> Document {
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.5);
        add_image(&mut d, png(8, 8, |_, _| 0), (8, 8), (10.0, 10.0), (20.0, 20.0), 3);
        d
    }

    #[test]
    fn a_black_image_burns_its_whole_area() {
        let d = black_doc();
        let plans = scan_plans_for_layer(&d, 3);
        assert_eq!(plans.len(), 1);
        let burnt = plans[0].burn_length() * plans[0].interval_mm;
        assert!((burnt - 100.0).abs() < 5.0, "area {burnt}");
        // Lines sit inside the image's box.
        let first = &plans[0].lines[0];
        let p = plans[0].point_mm(first, first.spans[0].runs[0].x0);
        assert!(p.x >= 14.9 && p.x <= 25.1 && p.y >= 14.9 && p.y <= 25.1, "{p:?}");
    }

    #[test]
    fn white_images_and_other_modes_give_nothing() {
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.5);
        add_image(&mut d, png(8, 8, |_, _| 255), (8, 8), (10.0, 10.0), (20.0, 20.0), 3);
        assert!(scan_plans_for_layer(&d, 3).iter().all(|p| p.burn_length() == 0.0));
        d.layer_mut(3).main_mut().mode = Mode::Fill;
        assert!(scan_plans_for_layer(&d, 3).is_empty());
        assert!(scan_plans_for_layer(&d, 4).is_empty());
    }

    #[test]
    fn layer_fields_reach_the_scan() {
        let mut d = black_doc();
        {
            let c = d.layer_mut(3).main_mut();
            c.negative = true;
        }
        assert!(scan_plans_for_layer(&d, 3).iter().all(|p| p.burn_length() == 0.0), "negative of black is nothing");
        {
            let c = d.layer_mut(3).main_mut();
            c.negative = false;
            c.max_power = 40.0;
            c.min_power = 10.0;
            c.overscan = 3.0;
            c.overscan_percent = false;
            c.bidirectional = false;
        }
        let plans = scan_plans_for_layer(&d, 3);
        let line = &plans[0].lines[0];
        assert!(line.spans[0].runs.iter().all(|r| (r.power - 40.0).abs() < 1e-3), "full black burns at max power");
        assert!(plans[0].lines.iter().all(|l| !l.reverse), "uni-directional");
        let span = &line.spans[0];
        assert!((span.runs[0].x0 - span.travel_start).abs() > 2.9, "3 mm overscan");
        // Dot width correction shortens the runs.
        let before: f64 = plans[0].burn_length();
        d.layer_mut(3).main_mut().dot_width_correction = 0.2;
        let after = scan_plans_for_layer(&d, 3)[0].burn_length();
        assert!(after < before);
    }

    #[test]
    fn pass_through_and_every_mode_run() {
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.25);
        add_image(&mut d, png(32, 32, |x, _| (x * 8) as u8), (32, 32), (8.0, 8.0), (10.0, 10.0), 3);
        for m in LayerMode::ALL {
            d.layer_mut(3).main_mut().image_mode = m;
            let burnt = scan_plans_for_layer(&d, 3).iter().map(ScanPlan::burn_length).sum::<f64>();
            assert!(burnt > 0.0, "{m:?} burns something");
        }
        d.layer_mut(3).main_mut().pass_through = true;
        assert!(scan_plans_for_layer(&d, 3)[0].burn_length() > 0.0);
        assert_eq!(engine_mode(d.layer(3).main()), ImageMode::PassThrough);
    }

    #[test]
    fn angle_and_pass_increment_rotate_the_scan() {
        let mut d = black_doc();
        {
            let c = d.layer_mut(3).main_mut();
            c.scan_angle = 10.0;
            c.angle_increment = 5.0;
        }
        assert_eq!(scan_plans_for_layer_pass(&d, 3, 0)[0].angle_deg, 10.0);
        assert_eq!(scan_plans_for_layer_pass(&d, 3, 2)[0].angle_deg, 20.0);
    }

    #[test]
    fn images_merge_into_one_scan_or_stay_apart() {
        let mut d = black_doc();
        add_image(&mut d, png(8, 8, |_, _| 0), (8, 8), (10.0, 10.0), (50.0, 20.0), 3);
        assert_eq!(scan_plans_for_layer(&d, 3).len(), 1);
        d.layer_mut(3).main_mut().merge_images = false;
        assert_eq!(scan_plans_for_layer(&d, 3).len(), 2);
    }

    #[test]
    fn a_crop_keeps_only_what_is_inside() {
        let mut d = black_doc();
        let full = scan_plans_for_layer(&d, 3)[0].burn_length();
        let id = d.iter().next().unwrap().id;
        if let ShapeKind::Image(img) = &mut d.get_mut(id).unwrap().kind {
            img.crop = Some(darkburn_core::kurbo::Rect::new(-5.0, -5.0, 0.0, 5.0).to_path(0.1));
        }
        let cropped = scan_plans_for_layer(&d, 3)[0].burn_length();
        assert!(cropped < full * 0.6 && cropped > full * 0.4, "{cropped} of {full}");
    }

    #[test]
    fn power_scale_and_tuning_apply() {
        let mut d = black_doc();
        let id = d.iter().next().unwrap().id;
        d.get_mut(id).unwrap().power_scale = 50.0;
        d.layer_mut(3).main_mut().max_power = 80.0;
        let plans = scan_plans_for_layer(&d, 3);
        assert!(plans[0].lines[0].spans[0].runs.iter().all(|r| (r.power - 40.0).abs() < 1e-3));
        let mut tuning = ScanTuning::default();
        tuning.offsets.set(100.0, 1.0);
        let shifted = try_scan_plans(&d, 3, 0, &tuning).unwrap();
        let plain = try_scan_plans(&d, 3, 0, &ScanTuning::default()).unwrap();
        assert_ne!(shifted, plain);
    }

    #[test]
    fn huge_images_are_refused_with_a_reason() {
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.001);
        add_image(&mut d, png(2, 2, |_, _| 0), (2, 2), (1000.0, 1000.0), (500.0, 500.0), 3);
        let e = try_scan_plans(&d, 3, 0, &ScanTuning::default()).unwrap_err();
        assert!(matches!(e, ImageError::TooLarge { .. }));
        assert!(e.to_string().contains("lower the DPI"));
        assert!(scan_plans_for_layer(&d, 3).is_empty());
    }

    #[test]
    fn missing_pixels_are_reported() {
        let mut d = black_doc();
        d.images.clear();
        let e = try_scan_plans(&d, 3, 0, &ScanTuning::default()).unwrap_err();
        assert!(matches!(e, ImageError::Unreadable(_)));
    }

    #[test]
    fn adjustment_mapping_and_limits() {
        let a = adjustments(&ImageAdjust { brightness: 20.0, enhance_amount: 150.0, enhance_denoise: 10.0, ..Default::default() });
        assert_eq!(a.brightness, 20.0);
        assert!((a.enhance_amount - 1.5).abs() < 1e-6);
        assert!(a.enhance_radius > 0.0, "sharpening without a radius still sharpens");
        assert!((a.enhance_denoise - 0.1).abs() < 1e-6);
        assert!(adjustments(&ImageAdjust::default()).is_neutral());
        assert!(check_import_size(4000, 3000).is_ok());
        assert!(check_import_size(200_000, 10).is_err());
        assert!(check_import_size(20_000, 20_000).is_err());
        assert!(memory_warning(1_000_000).is_none());
        assert!(memory_warning(100_000_000).is_some());
        assert!(memory_warning(MAX_DOTS + 1).unwrap().contains("Lower"));
    }

    #[test]
    fn transparent_pixels_burn_nothing() {
        let img = image::RgbaImage::from_fn(8, 8, |x, _| if x < 4 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.5);
        add_image(&mut d, bytes, (8, 8), (10.0, 10.0), (20.0, 20.0), 3);
        let burnt = scan_plans_for_layer(&d, 3)[0].burn_length() * 0.5;
        assert!((burnt - 50.0).abs() < 5.0, "{burnt}");
    }
}
