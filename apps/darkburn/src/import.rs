//! Importing artwork into the document: SVG (read with usvg) and bitmaps (PNG, JPEG, BMP, GIF,
//! TIFF, WebP, sized with `image`).
//!
//! Placement. An SVG's coordinates start at its top-left corner with Y down; they are scaled to
//! millimetres at the chosen SVG resolution (72 or 96 pixels per inch for files without units)
//! and flipped so the drawing keeps its place relative to the top-left corner of the work area.
//! When the result does not lie wholly on the work area it is centred on it instead. A bitmap is
//! centred on the work area. A drop point overrides both: the import is centred on it.
//!
//! Colours. Each path goes on the layer whose colour is closest to its stroke colour, or its
//! fill colour when it has no stroke (gradients use their first stop). Exact matches win.
//! Tool layers T1 and T2 take part only when "import to tool layers" is on.
//!
//! SVG details: group transforms are flattened into the paths, `<use>`/symbols and CSS are
//! resolved by usvg, text is turned into outlines with the system fonts, embedded PNG, JPEG,
//! GIF and WebP images become image shapes, and clip paths are applied to filled shapes and
//! images (as a crop). Stroke-only and open paths are cut where they cross the clip's edge and
//! keep their curves; the cut-off parts are dropped.

use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use darkburn_core::kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Size, Vec2};
use darkburn_core::layer::{LAYER_COUNT, layer_colour};
use darkburn_core::{Clipboard, ImageData, ImageShape, ImageStore, Shape, ShapeKind, T1, T2};
use darkburn_geom::FillRule;

/// Pixels per inch assumed for a bitmap whose file does not say (0.1 mm per pixel).
pub const DEFAULT_IMAGE_DPI: f64 = 254.0;
/// File extensions read as bitmaps.
pub const BITMAP_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"];
/// File extensions read as vector art.
pub const VECTOR_EXTENSIONS: &[&str] = &["svg"];
/// Curve tolerance when a clip path has to be applied, in mm.
const CLIP_TOLERANCE: f64 = 0.01;

/// How to import.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Pixels per inch of an SVG's user units (72 or 96).
    pub svg_dpi: f32,
    /// Colours may map to the tool layers T1 and T2.
    pub tool_layers: bool,
    /// The work area, in mm.
    pub page: Size,
    /// The layer bitmaps go on.
    pub image_layer: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { svg_dpi: 96.0, tool_layers: false, page: Size::new(400.0, 400.0), image_layer: 0 }
    }
}

/// Shapes read from one file, with the images they use.
#[derive(Clone, Debug, Default)]
pub struct Imported {
    pub shapes: Vec<Shape>,
    pub images: ImageStore,
}

impl Imported {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Bounding box of everything, in mm.
    pub fn bbox(&self) -> Option<Rect> {
        self.shapes.iter().map(Shape::bbox).reduce(|a, b| a.union(b))
    }

    pub fn translate(&mut self, v: Vec2) {
        for s in &mut self.shapes {
            s.transform = Affine::translate(v) * s.transform;
        }
    }

    /// Centre on `at` when given; else keep the place when it lies on the work area `page`,
    /// otherwise centre on the work area.
    pub fn place(&mut self, page: Size, at: Option<Point>) {
        let Some(bb) = self.bbox() else { return };
        let area = Rect::from_origin_size(Point::ZERO, page).inflate(1e-6, 1e-6);
        let target = match at {
            Some(p) => Some(p),
            None if area.contains(bb.origin()) && area.contains(Point::new(bb.x1, bb.y1)) => None,
            None => Some(area.center()),
        };
        if let Some(c) = target {
            self.translate(c - bb.center());
        }
    }

    /// Set the file a bitmap came from (for Refresh Image).
    pub fn set_source(&mut self, source: &str) {
        for s in &mut self.shapes {
            if let ShapeKind::Image(img) = &mut s.kind {
                img.source = Some(source.to_string());
            }
        }
    }

    /// As clipboard contents (paste from the system clipboard).
    pub fn into_clipboard(self) -> Clipboard {
        Clipboard { shapes: self.shapes, images: self.images, pastes: 0, cut_settings: None }
    }
}

/// What a file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Project,
    Svg,
    Bitmap,
    Other,
}

pub fn kind_of(path: &Path) -> FileKind {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == darkburn_core::project::EXTENSION {
        FileKind::Project
    } else if VECTOR_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Svg
    } else if BITMAP_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Bitmap
    } else {
        FileKind::Other
    }
}

/// Every extension Import accepts.
pub fn import_extensions() -> Vec<&'static str> {
    VECTOR_EXTENSIONS.iter().chain(BITMAP_EXTENSIONS).copied().collect()
}

/// Whether clipboard text is an SVG document.
pub fn is_svg_text(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with("<svg") || ((t.starts_with("<?xml") || t.starts_with("<!--") || t.starts_with("<!DOCTYPE")) && t.contains("<svg"))
}

/// Read a file's bytes as artwork (SVG or bitmap, by extension, else by content) and place it.
pub fn import_bytes(name: &Path, bytes: &[u8], opts: &Options, at: Option<Point>) -> Result<Imported, String> {
    let mut out = match kind_of(name) {
        FileKind::Svg => svg(bytes, opts)?,
        FileKind::Bitmap => bitmap(bytes, opts, None)?,
        FileKind::Project => return Err("a project opens with Open, not Import".into()),
        FileKind::Other => {
            if image::guess_format(bytes).is_ok() {
                bitmap(bytes, opts, None)?
            } else if std::str::from_utf8(&bytes[..bytes.len().min(4096)]).is_ok_and(is_svg_text) {
                svg(bytes, opts)?
            } else {
                return Err("this kind of file cannot be imported".into());
            }
        }
    };
    if out.is_empty() {
        return Err("there is nothing in it to import".into());
    }
    out.set_source(&name.to_string_lossy());
    out.place(opts.page, at);
    Ok(out)
}

/// The layer for colour `rgb`: an exact layer colour first, else the closest. Tool layers only
/// when `tool_layers` is on.
pub fn layer_for(rgb: [u8; 3], tool_layers: bool) -> usize {
    let allowed = |i: &usize| tool_layers || (*i != T1 && *i != T2);
    if let Some(exact) = (0..LAYER_COUNT).filter(allowed).find(|i| layer_colour(*i) == rgb) {
        return exact;
    }
    let d = |i: usize| -> i32 { layer_colour(i).iter().zip(rgb).map(|(a, b)| (*a as i32 - b as i32).pow(2)).sum() };
    (0..LAYER_COUNT).filter(allowed).min_by_key(|i| d(*i)).unwrap_or(0)
}

/// Keep a layer off the tool layers when that is not allowed.
pub fn allowed_layer(layer: usize, tool_layers: bool) -> usize {
    if !tool_layers && (layer == T1 || layer == T2) { 0 } else { layer.min(LAYER_COUNT - 1) }
}

// ----- SVG -----

/// System fonts for SVG text, loaded once on first use.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            // The generic families of an SVG ("sans-serif") must name fonts this computer
            // has, or its text is silently left out: Arial and its kin are not on every Linux.
            let serif = installed_family(&db, &["Times New Roman", "Liberation Serif", "DejaVu Serif", "Noto Serif", "Times", "FreeSerif"]);
            let sans = installed_family(&db, &["Arial", "Liberation Sans", "Helvetica", "DejaVu Sans", "Noto Sans", "FreeSans"]);
            let mono = installed_family(&db, &["Courier New", "Liberation Mono", "DejaVu Sans Mono", "Menlo", "Noto Sans Mono", "Courier"]);
            db.set_serif_family(serif.clone());
            db.set_sans_serif_family(sans.clone());
            db.set_monospace_family(mono);
            db.set_cursive_family(sans.clone());
            db.set_fantasy_family(sans);
            Arc::new(db)
        })
        .clone()
}

/// The first of `wanted` that is installed; failing all of them, any installed family.
fn installed_family(db: &usvg::fontdb::Database, wanted: &[&str]) -> String {
    let has = |name: &str| db.faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)));
    wanted
        .iter()
        .copied()
        .find(|n| has(n))
        .map(str::to_owned)
        .or_else(|| db.faces().find_map(|f| f.families.first().map(|(n, _)| n.clone())))
        .unwrap_or_else(|| wanted[0].to_owned())
}

fn affine(t: usvg::Transform) -> Affine {
    Affine::new([t.sx as f64, t.ky as f64, t.kx as f64, t.sy as f64, t.tx as f64, t.ty as f64])
}

fn to_bez(path: &usvg::tiny_skia_path::Path, m: Affine) -> BezPath {
    use usvg::tiny_skia_path::PathSegment;
    let p = |q: usvg::tiny_skia_path::Point| m * Point::new(q.x as f64, q.y as f64);
    let mut out = BezPath::new();
    for seg in path.segments() {
        match seg {
            PathSegment::MoveTo(a) => out.move_to(p(a)),
            PathSegment::LineTo(a) => out.line_to(p(a)),
            PathSegment::QuadTo(a, b) => out.quad_to(p(a), p(b)),
            PathSegment::CubicTo(a, b, c) => out.curve_to(p(a), p(b), p(c)),
            PathSegment::Close => out.close_path(),
        }
    }
    out
}

fn paint_rgb(paint: &usvg::Paint) -> [u8; 3] {
    let c = match paint {
        usvg::Paint::Color(c) => *c,
        usvg::Paint::LinearGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        usvg::Paint::RadialGradient(g) => g.stops().first().map(|s| s.color()).unwrap_or(usvg::Color::black()),
        usvg::Paint::Pattern(_) => usvg::Color::black(),
    };
    [c.red, c.green, c.blue]
}

struct SvgReader<'a> {
    opts: &'a Options,
    out: Imported,
}

/// A clip region in mm (filled by the non-zero rule after normalising).
type Clip = Vec<BezPath>;

fn all_closed(path: &BezPath) -> bool {
    let mut open = false;
    let mut closed_last = true;
    for el in path.elements() {
        match el {
            PathEl::MoveTo(_) => {
                if !closed_last {
                    open = true;
                }
                closed_last = false;
            }
            PathEl::ClosePath => closed_last = true,
            _ => {}
        }
    }
    !open && closed_last
}

/// Whether the clip is a plain rectangle that holds `bb` (then nothing needs cutting).
fn clip_holds(clip: &Clip, bb: Rect) -> bool {
    clip.iter().any(|c| {
        let cb = c.bounding_box();
        let rect_like = (c.area().abs() - cb.area()).abs() <= cb.area() * 1e-6 + 1e-9;
        rect_like && cb.inflate(1e-6, 1e-6).contains(bb.origin()) && cb.inflate(1e-6, 1e-6).contains(Point::new(bb.x1, bb.y1))
    })
}

fn clip_bbox(clip: &Clip) -> Option<Rect> {
    clip.iter().map(|c| c.bounding_box()).reduce(|a, b| a.union(b))
}

fn join(paths: Vec<BezPath>) -> BezPath {
    let mut all = BezPath::new();
    for p in paths {
        all.extend(p);
    }
    all
}

impl SvgReader<'_> {
    /// The clip region of `clip` used by an element whose coordinates map to mm by `m`.
    fn clip_region(&self, clip: &usvg::ClipPath, m: Affine) -> Clip {
        let m = m * affine(clip.transform());
        let mut paths = Vec::new();
        self.clip_paths(clip.root(), m * affine(clip.root().transform()), &mut paths);
        let mut region: Clip = darkburn_geom::boolean::union(&paths, FillRule::NonZero, CLIP_TOLERANCE);
        if let Some(inner) = clip.clip_path() {
            let other = self.clip_region(inner, m);
            region = darkburn_geom::boolean::intersection(&region, &other, FillRule::NonZero, CLIP_TOLERANCE);
        }
        region
    }

    /// The paths of a clip path's group. `m` maps the group's own coordinates to mm.
    fn clip_paths(&self, g: &usvg::Group, m: Affine, out: &mut Vec<BezPath>) {
        for node in g.children() {
            match node {
                usvg::Node::Group(g) => self.clip_paths(g, m * affine(g.transform()), out),
                usvg::Node::Path(p) => out.push(to_bez(p.data(), m)),
                usvg::Node::Text(t) => self.clip_paths(t.flattened(), m, out),
                usvg::Node::Image(_) => {}
            }
        }
    }

    /// Read group `g`. `m` maps the coordinates of the group's children to mm: every group's own
    /// transform is multiplied in on the way down. (usvg's ready-made absolute transforms are not
    /// used: for a `<use>` with a `transform` they hold that transform twice.)
    fn group(&mut self, g: &usvg::Group, m: Affine, clip: Option<&Clip>) {
        let own;
        let mut clip = clip;
        if let Some(cp) = g.clip_path() {
            let region = self.clip_region(cp, m);
            own = match clip {
                Some(outer) => darkburn_geom::boolean::intersection(outer, &region, FillRule::NonZero, CLIP_TOLERANCE),
                None => region,
            };
            clip = Some(&own);
        }
        for node in g.children() {
            match node {
                usvg::Node::Group(child) => self.group(child, m * affine(child.transform()), clip),
                usvg::Node::Path(p) => {
                    if !p.is_visible() {
                        continue;
                    }
                    self.path(p, m, clip);
                }
                usvg::Node::Text(t) => self.group(t.flattened(), m, clip),
                usvg::Node::Image(img) => {
                    if img.is_visible() {
                        self.image(img, m, clip);
                    }
                }
            }
        }
    }

    fn path(&mut self, p: &usvg::Path, m: Affine, clip: Option<&Clip>) {
        let rgb = match (p.stroke(), p.fill()) {
            (Some(s), _) => paint_rgb(s.paint()),
            (None, Some(f)) => paint_rgb(f.paint()),
            (None, None) => [0, 0, 0],
        };
        let layer = layer_for(rgb, self.opts.tool_layers);
        let mut path = to_bez(p.data(), m);
        if path.elements().is_empty() {
            return;
        }
        if let Some(clip) = clip {
            let bb = path.bounding_box();
            if !clip_holds(clip, bb) {
                let Some(cb) = clip_bbox(clip) else { return };
                if cb.intersect(bb).area() <= 0.0 && !(bb.width() == 0.0 || bb.height() == 0.0) {
                    return;
                }
                if all_closed(&path) && p.fill().is_some() {
                    let rule = match p.fill().map(|f| f.rule()) {
                        Some(usvg::FillRule::EvenOdd) => FillRule::EvenOdd,
                        _ => FillRule::NonZero,
                    };
                    let normal = darkburn_geom::boolean::resolve(&[path], rule, CLIP_TOLERANCE);
                    path = join(darkburn_geom::boolean::intersection(&normal, clip, FillRule::NonZero, CLIP_TOLERANCE));
                    if path.elements().is_empty() {
                        return;
                    }
                } else {
                    path = darkburn_geom::clip::clip_stroke(&path, clip, CLIP_TOLERANCE);
                    if path.elements().is_empty() {
                        return;
                    }
                }
            }
        }
        self.out.shapes.push(Shape::path(path, layer));
    }

    fn image(&mut self, img: &usvg::Image, m: Affine, clip: Option<&Clip>) {
        let size = img.size();
        let (bytes, format) = match img.kind() {
            usvg::ImageKind::PNG(b) => (b, "png"),
            usvg::ImageKind::JPEG(b) => (b, "jpg"),
            usvg::ImageKind::GIF(b) => (b, "gif"),
            usvg::ImageKind::WEBP(b) => (b, "webp"),
            usvg::ImageKind::SVG(tree) => {
                // A nested SVG: its own coordinates map through the image's transform.
                self.group(tree.root(), m * affine(tree.root().transform()), clip);
                return;
            }
        };
        let (pw, ph) = pixel_size(bytes).unwrap_or((size.width().round() as u32, size.height().round() as u32));
        let key = ImageData::key_for(bytes);
        self.out
            .images
            .entry(key.clone())
            .or_insert_with(|| Arc::new(ImageData { format: format.into(), bytes: Arc::from(&bytes[..]), px_width: pw, px_height: ph }));
        // Unit square (image v down) to mm.
        let a = m * Affine::scale_non_uniform(size.width() as f64, size.height() as f64);
        let c = a.as_coeffs();
        let (w, h) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let local = Affine::new([w, 0.0, 0.0, -h, -w / 2.0, h / 2.0]);
        let transform = a * local.inverse();
        let mut shape = ImageShape { image: key, width: w, height: h, ..Default::default() };
        if let Some(clip) = clip {
            let bb = (transform * Rect::from_center_size(Point::ZERO, (w, h)).to_path(0.1)).bounding_box();
            if !clip_holds(clip, bb) {
                let mut crop = join(clip.clone());
                crop.apply_affine(transform.inverse());
                shape.crop = Some(crop);
            }
        }
        let layer = allowed_layer(self.opts.image_layer, self.opts.tool_layers);
        self.out.shapes.push(Shape::new(ShapeKind::Image(shape), transform, layer));
    }
}

/// Read an SVG document. Coordinates are mm with the SVG's top-left at the work area's
/// top-left (not yet placed).
pub fn svg(data: &[u8], opts: &Options) -> Result<Imported, String> {
    let dpi = if opts.svg_dpi.is_finite() && opts.svg_dpi > 0.0 { opts.svg_dpi } else { 96.0 };
    let mut o = usvg::Options { dpi, ..Default::default() };
    if memchr_text(data) {
        o.fontdb = fonts();
    }
    let tree = usvg::Tree::from_data(data, &o).map_err(|e| format!("the SVG could not be read ({e})"))?;
    let k = 25.4 / dpi as f64;
    let to_mm = Affine::new([k, 0.0, 0.0, -k, 0.0, opts.page.height]);
    let mut reader = SvgReader { opts, out: Imported::default() };
    reader.group(tree.root(), to_mm * affine(tree.root().transform()), None);
    Ok(reader.out)
}

/// The document has a `<text` element (fonts are loaded only then).
fn memchr_text(data: &[u8]) -> bool {
    data.windows(5).any(|w| w == b"<text")
}

// ----- Bitmaps -----

fn pixel_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// The extension DarkBurn stores a bitmap format under.
fn format_ext(f: image::ImageFormat) -> Option<&'static str> {
    use image::ImageFormat as F;
    Some(match f {
        F::Png => "png",
        F::Jpeg => "jpg",
        F::Bmp => "bmp",
        F::Gif => "gif",
        F::Tiff => "tif",
        F::WebP => "webp",
        _ => return None,
    })
}

/// What [`bitmap`] reads from a bitmap's header before any pixels are decoded.
pub struct BitmapHeader {
    /// The extension the format is stored under.
    pub ext: &'static str,
    /// Size in pixels.
    pub px: (u32, u32),
    /// Size in mm as an import places it on a work area of `page`.
    pub mm: (f64, f64),
}

/// Checks a bitmap's format and picture size from its header (the import limits) and works out
/// its size in mm the way [`bitmap`] does, without decoding or copying the pixels.
pub fn bitmap_header(bytes: &[u8], page: Size, dpi: Option<f64>) -> Result<BitmapHeader, String> {
    let format = image::guess_format(bytes).map_err(|_| "this is not an image DarkBurn can read".to_string())?;
    let ext = format_ext(format).ok_or_else(|| format!("{format:?} images are not supported"))?;
    let (pw, ph) =
        image::ImageReader::with_format(Cursor::new(bytes), format).into_dimensions().map_err(|e| format!("the image could not be read ({e})"))?;
    if pw == 0 || ph == 0 {
        return Err("the image is empty".into());
    }
    crate::images::check_import_size(pw, ph)?;
    let dpi = file_dpi(bytes, format).or(dpi).unwrap_or(DEFAULT_IMAGE_DPI);
    let mut w = pw as f64 * 25.4 / dpi;
    let mut h = ph as f64 * 25.4 / dpi;
    let fit = (page.width / w).min(page.height / h).min(1.0);
    if fit.is_finite() && fit > 0.0 {
        w *= fit;
        h *= fit;
    }
    Ok(BitmapHeader { ext, px: (pw, ph), mm: (w, h) })
}

/// A bitmap as an image shape centred on the origin (not yet placed). Its size comes from the
/// file's own resolution, else `dpi`, else [`DEFAULT_IMAGE_DPI`]; it is scaled down to fit the
/// work area when larger.
pub fn bitmap(bytes: &[u8], opts: &Options, dpi: Option<f64>) -> Result<Imported, String> {
    let header = bitmap_header(bytes, opts.page, dpi)?;
    let (ext, pw, ph, w, h) = (header.ext, header.px.0, header.px.1, header.mm.0, header.mm.1);
    let data = ImageData { format: ext.into(), bytes: Arc::from(bytes), px_width: pw, px_height: ph };
    let key = ImageData::key_for(bytes);
    let mut out = Imported::default();
    out.images.insert(key.clone(), Arc::new(data));
    let layer = allowed_layer(opts.image_layer, opts.tool_layers);
    out.shapes.push(Shape::new(ShapeKind::Image(ImageShape { image: key, width: w, height: h, ..Default::default() }), Affine::IDENTITY, layer));
    Ok(out)
}

/// RGBA pixels (a pasted bitmap) as PNG bytes.
pub fn rgba_to_png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

fn sane_dpi(dpi: f64) -> Option<f64> {
    (dpi.is_finite() && (10.0..=20_000.0).contains(&dpi)).then_some(dpi)
}

/// The resolution a bitmap file states: PNG pHYs, JPEG JFIF or Exif, BMP header, TIFF tags.
pub fn file_dpi(bytes: &[u8], format: image::ImageFormat) -> Option<f64> {
    use image::ImageFormat as F;
    match format {
        F::Png => png_dpi(bytes),
        F::Jpeg => jpeg_dpi(bytes),
        F::Bmp => {
            let ppm = i32::from_le_bytes(bytes.get(38..42)?.try_into().ok()?);
            sane_dpi(ppm as f64 * 0.0254)
        }
        F::Tiff => tiff_dpi(bytes),
        _ => None,
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn png_dpi(b: &[u8]) -> Option<f64> {
    let mut at = 8;
    while at + 8 <= b.len() {
        let len = be32(b, at)? as usize;
        let kind = b.get(at + 4..at + 8)?;
        if kind == b"pHYs" {
            let x = be32(b, at + 8)?;
            let unit = *b.get(at + 16)?;
            return if unit == 1 { sane_dpi(x as f64 * 0.0254) } else { None };
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        at = at.checked_add(12 + len)?;
    }
    None
}

fn jpeg_dpi(b: &[u8]) -> Option<f64> {
    if b.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    let mut exif = None;
    while at + 4 <= b.len() && b[at] == 0xFF {
        let marker = b[at + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([b[at + 2], b[at + 3]]) as usize;
        let seg = b.get(at + 4..at + 2 + len)?;
        if marker == 0xE0 && seg.starts_with(b"JFIF\0") && seg.len() >= 12 {
            let units = seg[7];
            let x = u16::from_be_bytes([seg[8], seg[9]]) as f64;
            match units {
                1 => return sane_dpi(x),
                2 => return sane_dpi(x * 2.54),
                _ => {}
            }
        }
        if marker == 0xE1 && seg.starts_with(b"Exif\0\0") {
            exif = tiff_dpi(&seg[6..]);
        }
        at += 2 + len;
    }
    exif
}

/// X resolution from the first IFD of a TIFF (or an Exif block, which is one).
fn tiff_dpi(b: &[u8]) -> Option<f64> {
    let le = match b.get(0..4)? {
        [b'I', b'I', 42, 0] => true,
        [b'M', b'M', 0, 42] => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    let mut res = None;
    let mut unit = 2;
    for i in 0..count.min(512) {
        let e = ifd + 2 + i * 12;
        match u16_at(e)? {
            0x011A => {
                let off = u32_at(e + 8)? as usize;
                let (n, d) = (u32_at(off)?, u32_at(off + 4)?);
                if d != 0 {
                    res = Some(n as f64 / d as f64);
                }
            }
            0x0128 => unit = u16_at(e + 8)?,
            _ => {}
        }
    }
    match unit {
        2 => sane_dpi(res?),
        3 => sane_dpi(res? * 2.54),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use darkburn_core::layer::nearest_layer;

    fn opts() -> Options {
        Options { svg_dpi: 96.0, tool_layers: false, page: Size::new(400.0, 300.0), image_layer: 3 }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        rgba_to_png(w, h, vec![200; (w * h * 4) as usize]).unwrap()
    }

    #[test]
    fn svg_paths_are_scaled_flipped_and_keep_their_place() {
        let doc = br##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
            <g transform="translate(10,20)"><rect x="0" y="0" width="96" height="48" fill="none" stroke="#ff0000"/></g></svg>"##;
        let imp = svg(doc, &opts()).unwrap();
        assert_eq!(imp.shapes.len(), 1);
        let bb = imp.bbox().unwrap();
        // 96 px = 1 inch = 25.4 mm at 96 DPI; the group's translate is applied; Y flipped from the top.
        let k = 25.4 / 96.0;
        assert!((bb.x0 - 10.0 * k).abs() < 1e-6 && (bb.width() - 25.4).abs() < 1e-6, "{bb:?}");
        assert!((bb.y1 - (300.0 - 20.0 * k)).abs() < 1e-6 && (bb.height() - 12.7).abs() < 1e-6, "{bb:?}");
        assert_eq!(imp.shapes[0].layer, nearest_layer([255, 0, 0]));
        let mut placed = imp.clone();
        placed.place(Size::new(400.0, 300.0), None);
        assert_eq!(placed.bbox(), imp.bbox(), "art on the page keeps its place");
    }

    #[test]
    fn svg_resolution_setting_changes_the_size() {
        let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="72" height="72"><path d="M0 0 L72 0" stroke="black"/></svg>"#;
        let at72 = svg(doc, &Options { svg_dpi: 72.0, ..opts() }).unwrap().bbox().unwrap();
        let at96 = svg(doc, &opts()).unwrap().bbox().unwrap();
        assert!((at72.width() - 25.4).abs() < 1e-6);
        assert!((at96.width() - 19.05).abs() < 1e-6);
        // Real units do not depend on the setting.
        let mm =
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="50mm" viewBox="0 0 50 50"><path d="M0 0 L50 0" stroke="black"/></svg>"#;
        for dpi in [72.0, 96.0] {
            let w = svg(mm, &Options { svg_dpi: dpi, ..opts() }).unwrap().bbox().unwrap().width();
            assert!((w - 50.0).abs() < 1e-4, "{dpi}: {w}");
        }
    }

    #[test]
    fn art_off_the_page_is_centred_and_a_drop_point_wins() {
        let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="5000"><circle cx="4000" cy="4000" r="100" fill="blue"/></svg>"#;
        let o = opts();
        let imp = import_bytes(Path::new("x.svg"), doc, &o, None).unwrap();
        let c = imp.bbox().unwrap().center();
        assert!((c.x - 200.0).abs() < 1e-6 && (c.y - 150.0).abs() < 1e-6);
        let imp = import_bytes(Path::new("x.svg"), doc, &o, Some(Point::new(30.0, 40.0))).unwrap();
        let c = imp.bbox().unwrap().center();
        assert!((c.x - 30.0).abs() < 1e-6 && (c.y - 40.0).abs() < 1e-6);
    }

    #[test]
    fn colours_map_to_layers_and_tool_layers_need_the_option() {
        for i in [0, 1, 5, 17, 29] {
            assert_eq!(layer_for(layer_colour(i), false), i, "exact colour of layer {i}");
        }
        assert_ne!(layer_for(layer_colour(T1), false), T1);
        assert_ne!(layer_for(layer_colour(T2), false), T2);
        assert_eq!(layer_for(layer_colour(T1), true), T1);
        assert_eq!(layer_for([255, 0, 0], false), nearest_layer([255, 0, 0]));
        let [r, g, b] = layer_colour(T2);
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><path d="M0 0 L5 5" stroke="rgb({r},{g},{b})"/><rect width="4" height="4" fill="rgb({r},{g},{b})"/></svg>"#
        );
        let off = svg(doc.as_bytes(), &opts()).unwrap();
        assert!(off.shapes.iter().all(|s| s.layer != T1 && s.layer != T2));
        let on = svg(doc.as_bytes(), &Options { tool_layers: true, ..opts() }).unwrap();
        assert!(on.shapes.iter().all(|s| s.layer == T2));
        assert_eq!(allowed_layer(T1, false), 0);
        assert_eq!(allowed_layer(T1, true), T1);
    }

    #[test]
    fn fill_colour_is_used_without_a_stroke_and_symbols_resolve() {
        let doc = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100">
            <style>.a { fill: #00ff00 }</style>
            <defs><symbol id="s"><rect class="a" width="10" height="10"/></symbol></defs>
            <use xlink:href="#s" x="5" y="5"/><use xlink:href="#s" x="50" y="50"/></svg>"##;
        let imp = svg(doc, &opts()).unwrap();
        assert_eq!(imp.shapes.len(), 2);
        assert!(imp.shapes.iter().all(|s| s.layer == nearest_layer([0, 255, 0])));
    }

    #[test]
    fn clip_paths_cut_filled_shapes() {
        let doc = br##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
            <clipPath id="c"><rect width="48" height="96"/></clipPath>
            <g clip-path="url(#c)"><rect width="96" height="96" fill="black"/><path d="M0 10 L96 10" stroke="red"/></g>
            <g clip-path="url(#c)"><rect x="60" width="10" height="10" fill="black"/></g></svg>"##;
        let imp = svg(doc, &opts()).unwrap();
        assert_eq!(imp.shapes.len(), 2, "the wholly clipped square is gone, the open line stays");
        let filled = imp.shapes.iter().find(|s| s.is_closed()).unwrap().bbox();
        assert!((filled.width() - 12.7).abs() < 1e-3 && (filled.height() - 25.4).abs() < 1e-3, "{filled:?}");
        let line = imp.shapes.iter().find(|s| !s.is_closed()).unwrap().bbox();
        assert!((line.width() - 12.7).abs() < 1e-3, "the stroke-only line is cut by the clip too: {line:?}");
    }

    #[test]
    fn clipped_curves_and_circles_keep_their_curves_and_are_cut_at_the_edge() {
        let doc = br##"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96">
            <clipPath id="c"><rect width="48" height="96"/></clipPath>
            <g clip-path="url(#c)"><path d="M0 48 C 30 0, 66 0, 96 48" stroke="black" fill="none"/>
            <circle cx="48" cy="60" r="20" stroke="black" fill="none"/></g></svg>"##;
        let imp = svg(doc, &opts()).unwrap();
        assert_eq!(imp.shapes.len(), 2);
        for s in &imp.shapes {
            assert!(s.bbox().x1 <= 12.7 + 1e-3, "nothing past the clip edge: {:?}", s.bbox());
            assert!(s.outline().elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(..))), "still curves");
        }
    }

    #[test]
    fn embedded_images_become_image_shapes() {
        let data = png(4, 2);
        let b64 = base64_encode(&data);
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="96" height="96"><image x="0" y="0" width="96" height="48" xlink:href="data:image/png;base64,{b64}"/></svg>"#
        );
        let imp = svg(doc.as_bytes(), &opts()).unwrap();
        assert_eq!(imp.shapes.len(), 1);
        let ShapeKind::Image(img) = &imp.shapes[0].kind else { panic!("not an image") };
        assert!((img.width - 25.4).abs() < 1e-6 && (img.height - 12.7).abs() < 1e-6);
        let d = &imp.images[&img.image];
        assert_eq!((d.format.as_str(), d.px_width, d.px_height), ("png", 4, 2));
        let bb = imp.bbox().unwrap();
        assert!((bb.x0).abs() < 1e-6 && (bb.y1 - 300.0).abs() < 1e-6, "top-left stays at the page's top-left: {bb:?}");
        assert_eq!(imp.shapes[0].layer, 3);
    }

    #[test]
    fn svg_text_becomes_outlines_when_fonts_exist() {
        let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50"><text x="10" y="40" font-family="Arial, sans-serif" font-size="30" fill="black">Hi</text></svg>"#;
        let imp = svg(doc, &opts()).unwrap();
        if fonts().is_empty() {
            return;
        }
        assert!(!imp.is_empty(), "text was turned into paths");
        let bb = imp.bbox().unwrap();
        assert!(bb.x0 > 10.0 * 25.4 / 96.0 - 1.0 && bb.y1 < 300.0, "{bb:?}");
    }

    #[test]
    fn bitmaps_are_sized_from_their_resolution() {
        let o = opts();
        let data = png(254, 127);
        let imp = bitmap(&data, &o, None).unwrap();
        let ShapeKind::Image(img) = &imp.shapes[0].kind else { panic!() };
        assert!((img.width - 25.4).abs() < 1e-9 && (img.height - 12.7).abs() < 1e-9, "default 254 DPI");
        assert_eq!(imp.shapes[0].layer, 3);
        // A pHYs chunk saying 300 DPI.
        let mut with_phys = data[..33].to_vec();
        let ppm = (300.0f64 / 0.0254).round() as u32;
        let mut chunk = b"pHYs".to_vec();
        chunk.extend(ppm.to_be_bytes());
        chunk.extend(ppm.to_be_bytes());
        chunk.push(1);
        with_phys.extend(9u32.to_be_bytes());
        with_phys.extend(&chunk);
        with_phys.extend([0, 0, 0, 0]);
        with_phys.extend(&data[33..]);
        let dpi = file_dpi(&with_phys, image::ImageFormat::Png).unwrap();
        assert!((dpi - 300.0).abs() < 0.01);
        // Larger than the work area: scaled to fit.
        let big = png(5000, 100);
        let imp = bitmap(&big, &o, None).unwrap();
        assert!((imp.bbox().unwrap().width() - 400.0).abs() < 1e-6);
        let mut imp = import_bytes(Path::new("a.png"), &data, &o, None).unwrap();
        let c = imp.bbox().unwrap().center();
        assert!((c.x - 200.0).abs() < 1e-9 && (c.y - 150.0).abs() < 1e-9, "centred on the page");
        imp.set_source("a.png");
        assert!(bitmap(b"nope", &o, None).is_err());
    }

    #[test]
    fn every_bitmap_format_reads() {
        let img = image::RgbaImage::from_pixel(10, 6, image::Rgba([10, 20, 30, 255]));
        for (fmt, ext) in [
            (image::ImageFormat::Png, "png"),
            (image::ImageFormat::Jpeg, "jpg"),
            (image::ImageFormat::Bmp, "bmp"),
            (image::ImageFormat::Gif, "gif"),
            (image::ImageFormat::Tiff, "tif"),
            (image::ImageFormat::WebP, "webp"),
        ] {
            let mut bytes = Cursor::new(Vec::new());
            let dynimg = image::DynamicImage::ImageRgba8(img.clone());
            let dynimg = if fmt == image::ImageFormat::Jpeg { image::DynamicImage::ImageRgb8(dynimg.to_rgb8()) } else { dynimg };
            dynimg.write_to(&mut bytes, fmt).unwrap();
            let imp = import_bytes(Path::new(&format!("x.{ext}")), bytes.get_ref(), &opts(), None).unwrap();
            let d = imp.images.values().next().unwrap();
            assert_eq!((d.format.as_str(), d.px_width, d.px_height), (ext, 10, 6));
        }
    }

    #[test]
    fn jfif_bmp_and_tiff_resolutions_are_read() {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]));
        let mut bmp = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img.clone()).write_to(&mut bmp, image::ImageFormat::Bmp).unwrap();
        let mut bmp = bmp.into_inner();
        let ppm = (200.0f64 / 0.0254).round() as i32;
        bmp[38..42].copy_from_slice(&ppm.to_le_bytes());
        assert!((file_dpi(&bmp, image::ImageFormat::Bmp).unwrap() - 200.0).abs() < 0.01);
        // A minimal JFIF header: 150 DPI.
        let jfif = [0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0, 150, 0, 150, 0, 0, 0xFF, 0xD9];
        assert_eq!(file_dpi(&jfif, image::ImageFormat::Jpeg), Some(150.0));
        // A minimal little-endian TIFF with XResolution 600/1 and ResolutionUnit inch.
        let mut tif = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 2, 0];
        tif.extend([0x1A, 0x01, 5, 0, 1, 0, 0, 0, 38, 0, 0, 0]);
        tif.extend([0x28, 0x01, 3, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
        tif.extend([0, 0, 0, 0]);
        tif.extend([88, 2, 0, 0, 1, 0, 0, 0]);
        assert_eq!(file_dpi(&tif, image::ImageFormat::Tiff), Some(600.0));
    }

    #[test]
    fn kinds_and_svg_text_detection() {
        assert_eq!(kind_of(Path::new("a.DBRN")), FileKind::Project);
        assert_eq!(kind_of(Path::new("a.svg")), FileKind::Svg);
        assert_eq!(kind_of(Path::new("a.JPEG")), FileKind::Bitmap);
        assert_eq!(kind_of(Path::new("a.dxf")), FileKind::Other);
        assert!(is_svg_text("<svg xmlns='x'/>"));
        assert!(is_svg_text("<?xml version=\"1.0\"?>\n<svg/>"));
        assert!(!is_svg_text("hello <svg"));
        assert!(!is_svg_text("<?xml version=\"1.0\"?><html/>"));
    }

    fn base64_encode(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    // ----- Hand-computed geometry (bug hunt, pass 2) -----

    /// The bounding box of an import in the SVG's own pixel coordinates (Y down) at 96 DPI.
    fn px_box(doc: &[u8]) -> Rect {
        let imp = svg(doc, &opts()).unwrap();
        let bb = imp.bbox().unwrap();
        let k = 25.4 / 96.0;
        Rect::new(bb.x0 / k, (300.0 - bb.y1) / k, bb.x1 / k, (300.0 - bb.y0) / k)
    }

    fn box_is(got: Rect, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
        (got.x0 - x0).abs() < 1e-3 && (got.y0 - y0).abs() < 1e-3 && (got.x1 - x1).abs() < 1e-3 && (got.y1 - y1).abs() < 1e-3
    }

    #[test]
    fn svg_nested_and_skew_transforms_land_where_the_maths_says() {
        // (10,0): rotate 90 -> (0,10); scale 2 -> (0,20); translate -> (10,40). (0,0) -> (10,20).
        let nested = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
            <g transform="translate(10,20) scale(2)"><g transform="rotate(90)"><path d="M0 0 L10 0" stroke="black"/></g></g></svg>"#;
        let b = px_box(nested);
        assert!(box_is(b, 10.0, 20.0, 10.0, 40.0), "{b:?}");
        // skewX(45): x' = x + y. (0,10) -> (10,10), (5,0) stays.
        let skew = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
            <path transform="skewX(45)" d="M5 0 L0 10" stroke="black"/></svg>"#;
        let b = px_box(skew);
        assert!(box_is(b, 5.0, 0.0, 10.0, 10.0), "{b:?}");
        // skewY(45) inside matrix(1 0 0 1 3 4): (10,0) -> (10,10) -> (13,14).
        let skew_y = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
            <g transform="matrix(1 0 0 1 3 4)"><path transform="skewY(45)" d="M0 0 L10 0" stroke="black"/></g></svg>"#;
        let b = px_box(skew_y);
        assert!(box_is(b, 3.0, 4.0, 13.0, 14.0), "{b:?}");
        // rotate about a point: rotate(180, 50, 50) maps (0,0)-(10,20) to (90,80)-(100,100).
        let about = br#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">
            <rect transform="rotate(180 50 50)" width="10" height="20" stroke="black" fill="none"/></svg>"#;
        let b = px_box(about);
        assert!(box_is(b, 90.0, 80.0, 100.0, 100.0), "{b:?}");
    }

    #[test]
    fn svg_view_box_and_preserve_aspect_ratio() {
        let doc = |par: &str| {
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 50 50" {par}>
                <rect width="50" height="50" stroke="black" fill="none"/></svg>"#
            )
        };
        // Default xMidYMid meet: scale 2, centred across: 50..150 by 0..100.
        let b = px_box(doc("").as_bytes());
        assert!(box_is(b, 50.0, 0.0, 150.0, 100.0), "{b:?}");
        let b = px_box(doc(r#"preserveAspectRatio="none""#).as_bytes());
        assert!(box_is(b, 0.0, 0.0, 200.0, 100.0), "{b:?}");
        let b = px_box(doc(r#"preserveAspectRatio="xMaxYMid meet""#).as_bytes());
        assert!(box_is(b, 100.0, 0.0, 200.0, 100.0), "{b:?}");
        // Slice: scale 4, 200 x 200, bottom edges aligned: y from -100 to 100.
        let b = px_box(doc(r#"preserveAspectRatio="xMinYMax slice""#).as_bytes());
        assert!(box_is(b, 0.0, -100.0, 200.0, 100.0), "{b:?}");
        // A view box that does not start at zero.
        let off = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="10 20 50 50">
            <path d="M10 20 L60 70" stroke="black"/></svg>"#;
        let b = px_box(off);
        assert!(box_is(b, 0.0, 0.0, 100.0, 100.0), "{b:?}");
        // Only a view box: its numbers are the size.
        let only = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 40"><path d="M0 0 L30 40" stroke="black"/></svg>"#;
        let b = px_box(only);
        assert!(box_is(b, 0.0, 0.0, 30.0, 40.0), "{b:?}");
    }

    #[test]
    fn svg_percent_and_unit_lengths() {
        // 50% of 200 px wide, 25% of 80 px high; x in inches, y in points (1 pt = 96/72 px).
        let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="80">
            <rect x="1in" y="9pt" width="50%" height="25%" stroke="black" fill="none"/></svg>"#;
        let b = px_box(doc);
        assert!(box_is(b, 96.0, 12.0, 196.0, 32.0), "{b:?}");
        // Real units in mm, cm, pc on a page in mm with no view box.
        let units = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm">
            <rect x="10mm" y="2cm" width="1pc" height="5mm" stroke="black" fill="none"/></svg>"#;
        let bb = svg(units, &opts()).unwrap().bbox().unwrap();
        assert!((bb.x0 - 10.0).abs() < 1e-4 && (bb.width() - 25.4 / 6.0).abs() < 1e-4, "{bb:?}");
        assert!((bb.y1 - (300.0 - 20.0)).abs() < 1e-4 && (bb.height() - 5.0).abs() < 1e-4, "{bb:?}");
        // A size in mm with a view box in other numbers: 1 unit = 2 mm.
        let scaled = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="60mm" viewBox="0 0 50 30">
            <rect x="5" y="10" width="20" height="15" stroke="black" fill="none"/></svg>"#;
        for dpi in [72.0, 96.0] {
            let bb = svg(scaled, &Options { svg_dpi: dpi, ..opts() }).unwrap().bbox().unwrap();
            assert!((bb.x0 - 10.0).abs() < 1e-3 && (bb.width() - 40.0).abs() < 1e-3, "{dpi}: {bb:?}");
            assert!((bb.y1 - 280.0).abs() < 1e-3 && (bb.height() - 30.0).abs() < 1e-3, "{dpi}: {bb:?}");
        }
    }

    #[test]
    fn svg_use_loops_and_zero_radius_arcs_do_not_hang_or_crash() {
        let looped = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100">
            <g id="a"><use xlink:href="#b"/><path d="M1 2 L30 40" stroke="black"/></g>
            <g id="b"><use xlink:href="#a"/></g>
            <use id="c" xlink:href="#c"/></svg>"##;
        // Whatever is kept, the reader returns.
        let _ = svg(looped, &opts());
        // An arc with a zero radius is a straight line to its end point.
        let arcs = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <path d="M0 0 A0 0 0 0 1 10 20 A5 0 0 1 0 30 20" stroke="black" fill="none"/></svg>"#;
        let imp = svg(arcs, &opts()).unwrap();
        let b = px_box(arcs);
        assert!(box_is(b, 0.0, 0.0, 30.0, 20.0), "{b:?}");
        assert!(imp.shapes.iter().all(|s| s.bbox().width().is_finite() && s.bbox().height().is_finite()));
        // A use with its own place and a transform: (2,3)-(12,8) moved by translate(100,0) then x=5,y=6.
        let used = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="400" height="100">
            <defs><rect id="r" x="2" y="3" width="10" height="5" stroke="black" fill="none"/></defs>
            <use xlink:href="#r" x="5" y="6" transform="translate(100,0)"/></svg>"##;
        let b = px_box(used);
        assert!(box_is(b, 107.0, 9.0, 117.0, 14.0), "{b:?}");
    }
}
