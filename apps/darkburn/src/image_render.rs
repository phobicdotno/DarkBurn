//! Images on the canvas as they will burn: the layer's image mode applied (dither preview) and
//! the image adjustments live [IMG-11..IMG-15, IMG-24, CANVAS-28].
//!
//! The pixels are processed on a worker thread at the layer's real resolution (or coarser for
//! very large images, at most [`MAX_SIDE`] dots a side) and uploaded as a texture. The result is
//! cached per image shape and keyed by everything that changes the dots (pixels, crop,
//! adjustments, layer image fields, size), but not by where the shape is, so dragging and
//! rotating reuse the texture. While a new result is being computed the previous one stays on
//! screen, stretched to the shape. The scan angle does not show in the preview (the dots are
//! computed on the image's own grid).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use darkburn_core::kurbo::Affine;
use darkburn_core::{Document, ImageData, ImageShape, Mode, Shape};
use darkburn_raster::{Adjustments, GrayF, ImageMode, ImageSettings, process_image};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

use crate::images;

/// The most dots on a side of a canvas texture.
pub const MAX_SIDE: u32 = 2048;
/// Frames an unused texture is kept for.
const KEEP_FRAMES: u64 = 600;

/// Worker jobs started so far (tests count them).
static STARTED: AtomicUsize = AtomicUsize::new(0);

pub fn jobs_started() -> usize {
    STARTED.load(Ordering::Relaxed)
}

/// What to compute for one image.
#[derive(Clone)]
pub struct Job {
    pub data: Arc<ImageData>,
    pub shape: ImageShape,
    pub adjust: Adjustments,
    pub settings: ImageSettings,
    pub size_mm: (f64, f64),
    pub max_side: u32,
}

/// A processed picture of an image.
pub struct Rendered {
    pub image: ColorImage,
    /// The dots the picture is made of.
    pub dots: (u32, u32),
    /// The picture is coarser than the layer's resolution (the image has more dots than fit).
    pub reduced: bool,
}

/// Runs the image pipeline on `gray` (placed over `size_mm` millimetres) and returns what the
/// burn looks like: burnt dots dark on white, grey levels for grayscale.
pub fn render(gray: &GrayF, size_mm: (f64, f64), adjust: &Adjustments, settings: &ImageSettings, max_side: u32) -> Rendered {
    let (w_mm, h_mm) = (size_mm.0.max(1e-6), size_mm.1.max(1e-6));
    let interval = settings.line_interval_mm.max(1e-4);
    let longest = (w_mm / interval).max(h_mm / interval).max(1.0);
    let k = (f64::from(max_side) / longest).min(1.0);
    let mut s = settings.clone();
    s.line_interval_mm = interval / k;
    s.scan_angle_deg = 0.0;
    let to_mm = Affine::new([w_mm / f64::from(gray.width.max(1)), 0.0, 0.0, -h_mm / f64::from(gray.height.max(1)), 0.0, h_mm]);
    let p = process_image(gray, to_mm, adjust, &s);
    let preview = p.map.to_preview();
    let (w, h) = (preview.width(), preview.height());
    Rendered { image: ColorImage::from_gray([w as usize, h as usize], preview.as_raw()), dots: (w, h), reduced: k < 1.0 }
}

/// Decodes and renders a job.
pub fn compute(job: &Job) -> Option<ColorImage> {
    let gray = images::source_gray_of(&job.data, &job.shape).ok()?;
    Some(render(&gray, job.size_mm, &job.adjust, &job.settings, job.max_side).image)
}

/// How an image shape is shown: the settings its layer asks for, or plain grey with the
/// adjustments when the layer is not an image layer. None when it needs no processing.
pub fn display_settings(doc: &Document, leaf: &Shape, img: &ImageShape) -> Option<(Adjustments, ImageSettings)> {
    let layer = doc.layer(leaf.layer).main();
    let adjust = images::adjustments(&img.adjust);
    if layer.mode == Mode::Image {
        return Some((adjust, images::image_settings(layer, 0.0)));
    }
    if adjust.is_neutral() {
        return None;
    }
    // Not an image layer: show the tone changes without turning the picture into dots.
    let settings = ImageSettings { mode: ImageMode::Grayscale, line_interval_mm: 0.1, scan_angle_deg: 0.0, negative: false, linear_light: true };
    Some((adjust, settings))
}

/// Everything that changes the dots, as one number.
pub fn job_hash(img: &ImageShape, adjust: &Adjustments, settings: &ImageSettings, size_mm: (f64, f64)) -> u64 {
    let bucket = |v: f64| ((v / settings.line_interval_mm.max(1e-4)).max(1.0).ln() * 16.0).round() as i64;
    let mut h = DefaultHasher::new();
    img.image.hash(&mut h);
    format!("{:?}|{:?}|{:?}|{:?}", img.crop, adjust, settings, (img.width.to_bits(), img.height.to_bits())).hash(&mut h);
    (bucket(size_mm.0), bucket(size_mm.1)).hash(&mut h);
    h.finish()
}

type Slot = Arc<Mutex<Option<Option<ColorImage>>>>;

#[derive(Clone)]
struct Entry {
    hash: u64,
    texture: Option<TextureHandle>,
    pending: Option<(u64, Slot)>,
    used: u64,
}

type Cache = HashMap<u64, Entry>;

fn cache_id() -> egui::Id {
    egui::Id::new("darkburn_image_render_cache")
}

/// The processed texture of image shape `leaf`, or None when it needs none or the first one is
/// still being computed (the caller then draws the plain picture). Starts and collects the
/// worker jobs.
pub fn texture(ctx: &Context, doc: &Document, leaf: &Shape, img: &ImageShape) -> Option<TextureHandle> {
    let (adjust, settings) = display_settings(doc, leaf, img)?;
    let data = doc.images.get(&img.image)?.clone();
    let size = images::size_mm(leaf, img);
    let hash = job_hash(img, &adjust, &settings, size);
    let frame = ctx.cumulative_frame_nr();
    let mut cache: Cache = ctx.data_mut(|d| d.remove_temp(cache_id())).unwrap_or_default();
    cache.retain(|_, e| frame.saturating_sub(e.used) < KEEP_FRAMES);
    let entry = cache.entry(leaf.id.0).or_insert_with(|| Entry { hash: 0, texture: None, pending: None, used: frame });
    entry.used = frame;
    // Collect a finished job.
    if let Some((h, slot)) = entry.pending.clone() {
        let done = slot.lock().ok().and_then(|mut s| s.take());
        if let Some(result) = done {
            entry.pending = None;
            if let Some(colour) = result {
                let options = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, ..Default::default() };
                entry.texture = Some(ctx.load_texture(format!("db_processed_{}", leaf.id.0), colour, options));
                entry.hash = h;
            }
        }
    }
    // Start a job when what is shown is out of date and none is running.
    if entry.hash != hash && entry.pending.is_none() {
        let slot: Slot = Arc::new(Mutex::new(None));
        entry.pending = Some((hash, slot.clone()));
        let job = Job { data, shape: img.clone(), adjust, settings, size_mm: size, max_side: MAX_SIDE };
        let repaint = ctx.clone();
        STARTED.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            let out = compute(&job);
            if let Ok(mut s) = slot.lock() {
                *s = Some(out);
            }
            repaint.request_repaint();
        });
    }
    let texture = entry.texture.clone();
    ctx.data_mut(|d| d.insert_temp(cache_id(), cache));
    texture
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::testutil::*;
    use darkburn_core::kurbo::Vec2;
    use darkburn_core::{ImageAdjust, ShapeKind};

    fn ramp_doc() -> (Document, darkburn_core::ShapeId) {
        let mut d = Document::new();
        image_layer(&mut d, 3, 0.25);
        let id = add_image(&mut d, png(64, 8, |x, _| (x * 4) as u8), (64, 8), (16.0, 2.0), (20.0, 20.0), 3);
        (d, id)
    }

    fn wait_for(ctx: &Context, doc: &Document, id: darkburn_core::ShapeId) -> TextureHandle {
        for _ in 0..400 {
            let leaf = doc.get(id).unwrap();
            let ShapeKind::Image(img) = &leaf.kind else { panic!() };
            if let Some(t) = texture(ctx, doc, leaf, img) {
                return t;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no texture");
    }

    #[test]
    fn dithering_makes_dots_that_get_denser_towards_dark() {
        let g = GrayF::from_fn(64, 8, |x, _| x as f32 / 63.0);
        let s = ImageSettings { mode: ImageMode::Atkinson, line_interval_mm: 0.25, ..Default::default() };
        let r = render(&g, (16.0, 2.0), &Adjustments::default(), &s, 2048);
        assert!(!r.reduced);
        assert_eq!(r.dots.0, 64);
        let ink = |x0: usize, x1: usize| -> usize { (0..r.image.size[1]).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| r.image.pixels[y * r.image.size[0] + x].r() < 128).count() };
        let (left, right) = (ink(0, 16), ink(48, 64));
        assert!(left > right * 3, "dark side has more dots: {left} vs {right}");
        assert!(r.image.pixels.iter().all(|p| p.r() == 0 || p.r() == 255), "binary mode gives pure dots");
    }

    #[test]
    fn huge_images_are_reduced() {
        let g = GrayF::new(100, 100, 0.5);
        let s = ImageSettings { mode: ImageMode::Ordered, line_interval_mm: 0.01, ..Default::default() };
        let r = render(&g, (1000.0, 1000.0), &Adjustments::default(), &s, 512);
        assert!(r.reduced);
        assert!(r.dots.0 <= 520 && r.dots.1 <= 520, "{:?}", r.dots);
    }

    #[test]
    fn hash_ignores_position_but_not_what_changes_the_dots() {
        let (mut d, id) = ramp_doc();
        let leaf = d.get(id).unwrap().clone();
        let ShapeKind::Image(img) = &leaf.kind else { panic!() };
        let (a, s) = display_settings(&d, &leaf, img).unwrap();
        let base = job_hash(img, &a, &s, images::size_mm(&leaf, img));
        let mut moved = leaf.clone();
        moved.transform = Affine::translate(Vec2::new(90.0, 30.0)) * moved.transform;
        assert_eq!(job_hash(img, &a, &s, images::size_mm(&moved, img)), base, "moving reuses the picture");
        let mut img2 = img.clone();
        img2.adjust = ImageAdjust { brightness: 10.0, ..Default::default() };
        let (a2, _) = display_settings(&d, &Shape { kind: ShapeKind::Image(img2.clone()), ..leaf.clone() }, &img2).unwrap();
        assert_ne!(job_hash(&img2, &a2, &s, images::size_mm(&leaf, img)), base);
        d.layer_mut(3).main_mut().image_mode = darkburn_core::layer::ImageMode::Stucki;
        let (a3, s3) = display_settings(&d, &leaf, img).unwrap();
        assert_ne!(job_hash(img, &a3, &s3, images::size_mm(&leaf, img)), base);
        let mut bigger = leaf.clone();
        bigger.transform = Affine::scale(2.0) * bigger.transform;
        assert_ne!(job_hash(img, &a, &s, images::size_mm(&bigger, img)), base, "a different size is different dots");
    }

    #[test]
    fn plain_pictures_on_other_layers_need_no_processing() {
        let (mut d, id) = ramp_doc();
        d.layer_mut(3).main_mut().mode = Mode::Line;
        let leaf = d.get(id).unwrap().clone();
        let ShapeKind::Image(img) = &leaf.kind else { panic!() };
        assert!(display_settings(&d, &leaf, img).is_none());
        let mut tweaked = img.clone();
        tweaked.adjust.gamma = 2.0;
        let (_, s) = display_settings(&d, &leaf, &tweaked).unwrap();
        assert_eq!(s.mode, ImageMode::Grayscale, "adjustments show live without dots");
    }

    #[test]
    fn the_texture_is_cached_while_dragging_and_rebuilt_when_it_changes() {
        let ctx = Context::default();
        let (mut d, id) = ramp_doc();
        let first = wait_for(&ctx, &d, id);
        let started = jobs_started();
        // Drag it: the same texture, no new work.
        d.get_mut(id).unwrap().transform = Affine::translate(Vec2::new(60.0, 10.0)) * d.get(id).unwrap().transform;
        for _ in 0..10 {
            let leaf = d.get(id).unwrap();
            let ShapeKind::Image(img) = &leaf.kind else { panic!() };
            assert_eq!(texture(&ctx, &d, leaf, img).unwrap().id(), first.id());
        }
        assert_eq!(jobs_started(), started, "moving starts no job");
        // Change the brightness: the old texture stays until the new one is ready.
        if let ShapeKind::Image(img) = &mut d.get_mut(id).unwrap().kind {
            img.adjust.brightness = 30.0;
        }
        let leaf = d.get(id).unwrap();
        let ShapeKind::Image(img) = &leaf.kind else { panic!() };
        let during = texture(&ctx, &d, leaf, img).expect("the old picture is kept meanwhile");
        assert_eq!(during.id(), first.id());
        assert_eq!(jobs_started(), started + 1);
        for _ in 0..400 {
            let leaf = d.get(id).unwrap();
            let ShapeKind::Image(img) = &leaf.kind else { panic!() };
            if texture(&ctx, &d, leaf, img).is_some_and(|t| t.id() != first.id()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the new picture never arrived");
    }
}
