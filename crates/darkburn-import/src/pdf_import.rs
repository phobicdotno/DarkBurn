//! PDF and PDF-compatible Adobe Illustrator import (object model via `lopdf`, MIT; the
//! content-stream interpreter is our own).
//!
//! Reads vector paths with the full transform stack (`q`/`Q`/`cm`, form XObjects), stroke and
//! fill colours in gray, RGB, CMYK, ICC-based, indexed and separation spaces, images (XObject
//! and uncompressed inline; Flate/LZW/RunLength/ASCII, JPEG, CCITT G3/G4, 1-bit masks, soft
//! masks) and optional-content layers (Illustrator layers, with hidden state). Clip paths are
//! not applied (a warning says so) and PDF text that was not converted to outlines is skipped.

use std::collections::BTreeMap;

use image::{DynamicImage, GrayImage, RgbImage, RgbaImage};
use kurbo::{Affine, BezPath, Point};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::model::*;
use crate::{ImportError, guard};

/// Which pages to import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageSelection {
    /// One page, 1-based.
    Page(usize),
    /// Every page, each in its own group, on top of each other.
    All,
}

/// PDF / AI import options.
#[derive(Clone, Debug)]
pub struct PdfOptions {
    pub pages: PageSelection,
    /// Keep items on hidden (switched-off) layers, marked `hidden`. When false they are dropped.
    pub include_hidden_layers: bool,
    pub include_images: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions { pages: PageSelection::Page(1), include_hidden_layers: true, include_images: true }
    }
}

const PT_TO_MM: f64 = 25.4 / 72.0;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_DECODED: usize = 512 * 1024 * 1024;
const MAX_FORM_DEPTH: u32 = 16;

fn load(bytes: &[u8]) -> Result<Document, ImportError> {
    guard("PDF", || Document::load_mem(bytes))?.map_err(|e| ImportError::Parse(format!("PDF: {e}")))
}

/// Number of pages in a PDF.
pub fn page_count(bytes: &[u8]) -> Result<usize, ImportError> {
    let doc = load(bytes)?;
    guard("PDF", || doc.get_pages().len())
}

/// Imports an Adobe Illustrator file. PDF-compatible files (the default since Illustrator 9)
/// are read as PDF; older PostScript-only files are reported as unsupported.
pub fn import_ai(bytes: &[u8], opts: &PdfOptions) -> Result<Imported, ImportError> {
    let head = &bytes[..bytes.len().min(4096)];
    let has_pdf = head.windows(5).any(|w| w == b"%PDF-");
    if has_pdf {
        let start = head.windows(5).position(|w| w == b"%PDF-").unwrap_or(0);
        let mut out = import(&bytes[start..], opts)?;
        if out.items.is_empty() {
            out.warn("The Illustrator file has no PDF content; re-save it with \"Create PDF Compatible File\" switched on");
        }
        return Ok(out);
    }
    if head.starts_with(b"%!PS") || head.windows(4).any(|w| w == b"%!PS") {
        return Err(ImportError::Unsupported(
            "This Adobe Illustrator file is PostScript-only (Illustrator 8 or older, or saved without PDF compatibility). \
             Re-save it from Illustrator with \"Create PDF Compatible File\" switched on, or export it as PDF or SVG."
                .into(),
        ));
    }
    Err(ImportError::Parse("not an Adobe Illustrator file".into()))
}

/// Imports a PDF.
pub fn import(bytes: &[u8], opts: &PdfOptions) -> Result<Imported, ImportError> {
    let doc = load(bytes)?;
    let pages = guard("PDF", || doc.get_pages())?;
    let mut out = Imported { page_count: Some(pages.len()), ..Default::default() };
    if pages.is_empty() {
        return Err(ImportError::Parse("the PDF has no pages".into()));
    }
    let off = hidden_ocgs(&doc);
    let selected: Vec<(u32, ObjectId)> = match opts.pages {
        PageSelection::All => pages.iter().map(|(n, id)| (*n, *id)).collect(),
        PageSelection::Page(n) => {
            let n = n.max(1) as u32;
            match pages.get(&n) {
                Some(id) => vec![(n, *id)],
                None => return Err(ImportError::Parse(format!("page {n} does not exist (the PDF has {} pages)", pages.len()))),
            }
        }
    };
    for (num, page_id) in selected {
        let group = if opts.pages == PageSelection::All { Some(out.new_group(None, Some(format!("Page {num}")))) } else { None };
        let mut interp = Interp { doc: &doc, opts, out: &mut out, off: &off, group, budget: 2_000_000, ops: 0 };
        let _ = guard("PDF page", || interp.page(page_id));
    }
    Ok(crate::finish(out))
}

/// Optional-content groups that are switched off by default.
fn hidden_ocgs(doc: &Document) -> Vec<ObjectId> {
    let mut v = Vec::new();
    let Some(cat) = doc.trailer.get(b"Root").ok().and_then(|r| deref(doc, r).as_dict().ok()) else { return v };
    let Some(ocp) = dict_get(doc, cat, b"OCProperties").and_then(|o| o.as_dict().ok()) else { return v };
    let Some(d) = dict_get(doc, ocp, b"D").and_then(|o| o.as_dict().ok()) else { return v };
    if let Some(arr) = dict_get(doc, d, b"OFF").and_then(|o| o.as_array().ok()) {
        for o in arr {
            if let Ok(id) = o.as_reference() {
                v.push(id);
            }
        }
    }
    v
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(f64::from(*r)).filter(|v| v.is_finite()),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(num).collect()
}

fn name(o: &Object) -> Option<&[u8]> {
    o.as_name().ok()
}

#[derive(Clone, Debug, PartialEq)]
enum Cs {
    Gray,
    Rgb,
    Cmyk,
    Lab,
    Indexed {
        base: Box<Cs>,
        hival: usize,
        lookup: Vec<u8>,
    },
    /// Separation / DeviceN: tint(s) rendered as darkness.
    Tint(usize),
    Pattern,
}

impl Cs {
    fn components(&self) -> usize {
        match self {
            Cs::Gray | Cs::Indexed { .. } | Cs::Pattern => 1,
            Cs::Rgb | Cs::Lab => 3,
            Cs::Cmyk => 4,
            Cs::Tint(n) => *n,
        }
    }

    /// Colour from component values in 0..=1 (indexed: the index value).
    fn to_rgb(&self, c: &[f64]) -> Rgb {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0);
        match self {
            Cs::Gray => Rgb::from_unit(g(0), g(0), g(0)),
            Cs::Rgb => Rgb::from_unit(g(0), g(1), g(2)),
            Cs::Cmyk => {
                let k = g(3);
                Rgb::from_unit((1.0 - g(0)) * (1.0 - k), (1.0 - g(1)) * (1.0 - k), (1.0 - g(2)) * (1.0 - k))
            }
            Cs::Lab => {
                let l = (g(0) / 100.0).clamp(0.0, 1.0);
                Rgb::from_unit(l, l, l)
            }
            Cs::Indexed { base, hival, lookup } => {
                let idx = (g(0).round().max(0.0) as usize).min(*hival);
                let n = base.components();
                let start = idx * n;
                let comps: Vec<f64> = (0..n).map(|k| f64::from(*lookup.get(start + k).unwrap_or(&0)) / 255.0).collect();
                base.to_rgb(&comps)
            }
            Cs::Tint(n) => {
                let t = (0..*n).map(g).fold(0.0f64, f64::max);
                Rgb::from_unit(1.0 - t, 1.0 - t, 1.0 - t)
            }
            Cs::Pattern => Rgb::BLACK,
        }
    }

    fn default_colour(&self) -> Vec<f64> {
        match self {
            Cs::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Cs::Tint(n) => vec![1.0; *n],
            _ => vec![0.0; self.components()],
        }
    }
}

#[derive(Clone)]
struct GState {
    ctm: Affine,
    stroke: Rgb,
    fill: Rgb,
    stroke_cs: Cs,
    fill_cs: Cs,
    dash: Option<Vec<f64>>,
}

struct Interp<'a> {
    doc: &'a Document,
    opts: &'a PdfOptions,
    out: &'a mut Imported,
    off: &'a [ObjectId],
    group: Option<u32>,
    budget: usize,
    ops: usize,
}

/// Resource dictionaries in lookup order (innermost first).
#[derive(Clone)]
struct Res<'a> {
    dicts: Vec<&'a Dictionary>,
}

impl<'a> Res<'a> {
    fn lookup(&self, doc: &'a Document, category: &[u8], key: &[u8]) -> Option<(&'a Object, Option<ObjectId>)> {
        for d in &self.dicts {
            if let Some(cat) = dict_get(doc, d, category).and_then(|o| o.as_dict().ok())
                && let Ok(raw) = cat.get(key)
            {
                let id = raw.as_reference().ok();
                return Some((deref(doc, raw), id));
            }
        }
        None
    }
}

struct PathBuilder {
    path: BezPath,
    current: Option<Point>,
    start: Option<Point>,
    /// The last part was closed: the next segment starts a new part at the current point.
    reopen: bool,
}

impl<'a> Interp<'a> {
    fn page(&mut self, page_id: ObjectId) {
        let doc = self.doc;
        let Ok(page) = doc.get_dictionary(page_id) else { return };
        let mbox = inherited(doc, page, b"CropBox").or_else(|| inherited(doc, page, b"MediaBox"));
        let rect = mbox.and_then(|o| o.as_array().ok()).map(|a| nums(a)).filter(|v| v.len() == 4).unwrap_or(vec![0.0, 0.0, 612.0, 792.0]);
        let (x0, y0) = (rect[0].min(rect[2]), rect[1].min(rect[3]));
        let (w, h) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
        let rotate = inherited(doc, page, b"Rotate").and_then(num).unwrap_or(0.0) as i64;
        let rot = match rotate.rem_euclid(360) {
            90 => Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, w]),
            180 => Affine::new([-1.0, 0.0, 0.0, -1.0, w, h]),
            270 => Affine::new([0.0, 1.0, -1.0, 0.0, h, 0.0]),
            _ => Affine::IDENTITY,
        };
        // UserUnit (PDF 1.6): the size of one unit in 1/72 inch, for pages larger than 200 inches.
        let unit = dict_get(doc, page, b"UserUnit").and_then(num).filter(|u| *u > 0.0).unwrap_or(1.0);
        let base = Affine::scale(PT_TO_MM * unit) * rot * Affine::translate((-x0, -y0));
        let mut dicts = Vec::new();
        if let Ok((inline, ids)) = doc.get_page_resources(page_id) {
            if let Some(d) = inline {
                dicts.push(d);
            }
            for id in ids {
                if let Ok(d) = doc.get_dictionary(id) {
                    dicts.push(d);
                }
            }
        }
        let content = doc.get_page_content_with_limit(page_id, MAX_DECODED).unwrap_or_default();
        let gs = GState { ctm: base, stroke: Rgb::BLACK, fill: Rgb::BLACK, stroke_cs: Cs::Gray, fill_cs: Cs::Gray, dash: None };
        self.run(&content, Res { dicts }, gs, 0, Vec::new());
    }

    fn run(&mut self, content: &[u8], res: Res<'a>, gs0: GState, depth: u32, mc0: Vec<Option<(String, bool)>>) {
        let ops: Vec<Operation> = match Content::decode(content) {
            Ok(c) => c.operations,
            Err(_) => {
                let mut v = Vec::new();
                for op in Content::decode_iter(content) {
                    match op {
                        Ok(o) => v.push(o),
                        Err(_) => break,
                    }
                }
                if !v.is_empty() {
                    self.out.warn("Part of a damaged PDF page could not be read");
                }
                v
            }
        };
        let doc = self.doc;
        let mut gs = gs0;
        let mut stack: Vec<GState> = Vec::new();
        let mut pb = PathBuilder { path: BezPath::new(), current: None, start: None, reopen: false };
        let mut clip_pending = false;
        // Marked-content stack: Some((layer name, hidden)) for optional content.
        let mut mc: Vec<Option<(String, bool)>> = mc0;
        for op in &ops {
            self.ops += 1;
            if self.ops > 50_000_000 || self.budget == 0 {
                self.out.warn("The PDF is too large; the rest was left out");
                break;
            }
            let a = &op.operands;
            let n = nums(a);
            let t = |gs: &GState, x: f64, y: f64| gs.ctm * Point::new(x, y);
            match op.operator.as_str() {
                "q" => {
                    if stack.len() < 256 {
                        stack.push(gs.clone());
                    }
                }
                "Q" => {
                    if let Some(s) = stack.pop() {
                        gs = s;
                    }
                }
                "cm" if n.len() == 6 => {
                    let m = Affine::new([n[0], n[1], n[2], n[3], n[4], n[5]]);
                    gs.ctm *= m;
                }
                "m" if n.len() >= 2 => {
                    let p = t(&gs, n[0], n[1]);
                    pb.path.move_to(p);
                    pb.current = Some(p);
                    pb.start = Some(p);
                    pb.reopen = false;
                }
                "l" if n.len() >= 2 => {
                    let p = t(&gs, n[0], n[1]);
                    ensure_start(&mut pb);
                    pb.path.line_to(p);
                    pb.current = Some(p);
                }
                "c" if n.len() >= 6 => {
                    ensure_start(&mut pb);
                    let p3 = t(&gs, n[4], n[5]);
                    pb.path.curve_to(t(&gs, n[0], n[1]), t(&gs, n[2], n[3]), p3);
                    pb.current = Some(p3);
                }
                "v" if n.len() >= 4 => {
                    ensure_start(&mut pb);
                    let c0 = pb.current.unwrap_or(Point::ZERO);
                    let p3 = t(&gs, n[2], n[3]);
                    pb.path.curve_to(c0, t(&gs, n[0], n[1]), p3);
                    pb.current = Some(p3);
                }
                "y" if n.len() >= 4 => {
                    ensure_start(&mut pb);
                    let p3 = t(&gs, n[2], n[3]);
                    pb.path.curve_to(t(&gs, n[0], n[1]), p3, p3);
                    pb.current = Some(p3);
                }
                "h" => {
                    if pb.current.is_some() {
                        pb.path.close_path();
                        pb.current = pb.start;
                        pb.reopen = true;
                    }
                }
                "re" if n.len() >= 4 => {
                    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
                    pb.path.move_to(t(&gs, x, y));
                    pb.path.line_to(t(&gs, x + w, y));
                    pb.path.line_to(t(&gs, x + w, y + h));
                    pb.path.line_to(t(&gs, x, y + h));
                    pb.path.close_path();
                    pb.current = Some(t(&gs, x, y));
                    pb.start = pb.current;
                    pb.reopen = true;
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    let o = op.operator.as_str();
                    if matches!(o, "s" | "b" | "b*") && pb.current.is_some() {
                        pb.path.close_path();
                    }
                    let stroke = matches!(o, "S" | "s" | "B" | "B*" | "b" | "b*");
                    let fill = matches!(o, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    if clip_pending {
                        self.out.warn("Clipping paths are not applied; clipped artwork is imported whole");
                        clip_pending = false;
                    }
                    let path = std::mem::take(&mut pb.path);
                    pb.current = None;
                    pb.start = None;
                    pb.reopen = false;
                    if (stroke || fill) && path.elements().len() > 1 && path.is_finite() {
                        let mut item = Item::path(path, stroke.then_some(gs.stroke), fill.then_some(gs.fill));
                        if let ItemKind::Path(p) = &mut item.kind {
                            p.dash = gs.dash.clone();
                        }
                        self.emit(item, &mc);
                    }
                }
                "W" | "W*" => clip_pending = true,
                "d" => {
                    let arr: Vec<f64> = a.first().and_then(|o| o.as_array().ok()).map(|v| nums(v)).unwrap_or_default();
                    let scale = gs.ctm.determinant().abs().sqrt();
                    gs.dash = if arr.is_empty() || arr.iter().all(|v| *v == 0.0) { None } else { Some(arr.iter().map(|v| v * scale).collect()) };
                }
                "CS" | "cs" => {
                    let cs = a.first().and_then(name).map(|nm| self.colour_space_named(nm, &res)).unwrap_or(Cs::Gray);
                    let col = cs.to_rgb(&cs.default_colour());
                    if op.operator == "CS" {
                        gs.stroke_cs = cs;
                        gs.stroke = col;
                    } else {
                        gs.fill_cs = cs;
                        gs.fill = col;
                    }
                }
                "SC" | "SCN" => gs.stroke = gs.stroke_cs.to_rgb(&n),
                "sc" | "scn" => gs.fill = gs.fill_cs.to_rgb(&n),
                "G" | "g" | "RG" | "rg" | "K" | "k" => {
                    let cs = match op.operator.to_ascii_uppercase().as_str() {
                        "G" => Cs::Gray,
                        "RG" => Cs::Rgb,
                        _ => Cs::Cmyk,
                    };
                    let col = cs.to_rgb(&n);
                    if op.operator.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                        gs.stroke = col;
                        gs.stroke_cs = cs;
                    } else {
                        gs.fill = col;
                        gs.fill_cs = cs;
                    }
                }
                "BMC" => mc.push(None),
                "BDC" => {
                    let entry = if a.first().and_then(name) == Some(b"OC") { a.get(1).and_then(|p| self.ocg_layer(p, &res)) } else { None };
                    mc.push(entry);
                }
                "EMC" => {
                    mc.pop();
                }
                "Do" => {
                    if let Some(nm) = a.first().and_then(name) {
                        self.xobject(nm, &res, &gs, depth, &mc);
                    }
                }
                "BI" => {
                    if let Some(Object::Stream(s)) = a.first() {
                        if self.opts.include_images {
                            let img = decode_image(doc, &s.dict, &s.content, gs.fill, &res, true);
                            self.place_image(img, &gs, &mc);
                        }
                    } else {
                        self.out.warn("Compressed inline images are not supported and were skipped");
                    }
                }
                "Tj" | "TJ" | "'" | "\"" => self.out.warn("PDF text is skipped; convert text to outlines before exporting the PDF"),
                "sh" => self.out.warn("Gradient shadings are not imported"),
                _ => {}
            }
        }
    }

    fn emit(&mut self, mut item: Item, mc: &[Option<(String, bool)>]) {
        let hidden = mc.iter().flatten().any(|(_, h)| *h);
        if hidden && !self.opts.include_hidden_layers {
            return;
        }
        if let Some((layer, _)) = mc.iter().rev().flatten().next() {
            item.layer = Some(layer.clone());
        }
        item.hidden = hidden;
        item.group = self.group;
        if self.budget > 0 {
            self.budget -= 1;
            self.out.items.push(item);
        }
    }

    /// Layer name and hidden state of an optional-content property (name or inline dict).
    fn ocg_layer(&self, prop: &Object, res: &Res<'a>) -> Option<(String, bool)> {
        let doc = self.doc;
        let (obj, id) = match prop {
            Object::Name(nm) => res.lookup(doc, b"Properties", nm)?,
            other => (deref(doc, other), other.as_reference().ok()),
        };
        let d = obj.as_dict().ok()?;
        // OCMD: take its first OCG.
        if d.get(b"Type").ok().and_then(name) == Some(b"OCMD") {
            let ocgs = d.get(b"OCGs").ok()?;
            let first = match deref(doc, ocgs) {
                Object::Array(a) => a.first()?,
                _ => ocgs,
            };
            return self.ocg_layer(first, res);
        }
        let label = dict_get(doc, d, b"Name").and_then(|n| n.as_str().ok()).map(pdf_string).unwrap_or_else(|| "Layer".into());
        let hidden = id.is_some_and(|i| self.off.contains(&i));
        Some((label, hidden))
    }

    fn colour_space_named(&self, nm: &[u8], res: &Res<'a>) -> Cs {
        match nm {
            b"DeviceGray" | b"G" | b"CalGray" => Cs::Gray,
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Cs::Rgb,
            b"DeviceCMYK" | b"CMYK" => Cs::Cmyk,
            b"Pattern" => Cs::Pattern,
            _ => match res.lookup(self.doc, b"ColorSpace", nm) {
                Some((o, _)) => colour_space(self.doc, o, res, 0),
                None => Cs::Gray,
            },
        }
    }

    fn xobject(&mut self, nm: &[u8], res: &Res<'a>, gs: &GState, depth: u32, mc: &[Option<(String, bool)>]) {
        let doc = self.doc;
        let Some((obj, _)) = res.lookup(doc, b"XObject", nm) else { return };
        let Ok(stream) = obj.as_stream() else { return };
        let subtype = stream.dict.get(b"Subtype").ok().and_then(name).unwrap_or(b"");
        match subtype {
            b"Form" => {
                if depth >= MAX_FORM_DEPTH {
                    self.out.warn("Nested PDF forms went too deep and were cut off");
                    return;
                }
                let m = stream.dict.get(b"Matrix").ok().and_then(|o| o.as_array().ok()).map(|a| nums(a)).filter(|v| v.len() == 6);
                let mut g = gs.clone();
                if let Some(m) = m {
                    g.ctm *= Affine::new([m[0], m[1], m[2], m[3], m[4], m[5]]);
                }
                let mut inner = res.clone();
                if let Some(d) = dict_get(doc, &stream.dict, b"Resources").and_then(|o| o.as_dict().ok()) {
                    inner.dicts.insert(0, d);
                }
                let content = stream.decompressed_content_with_limit(MAX_DECODED).unwrap_or_else(|_| stream.content.clone());
                // Optional content on the form itself.
                let mut mc2 = mc.to_vec();
                if let Ok(oc) = stream.dict.get(b"OC") {
                    mc2.push(self.ocg_layer(oc, res));
                }
                self.run(&content, inner, g, depth + 1, mc2);
            }
            b"Image" => {
                if !self.opts.include_images {
                    return;
                }
                let img = decode_image(doc, &stream.dict, &stream.content, gs.fill, res, false);
                self.place_image(img, gs, mc);
            }
            _ => {}
        }
    }

    fn place_image(&mut self, img: Result<DynamicImage, String>, gs: &GState, mc: &[Option<(String, bool)>]) {
        match img {
            Ok(image) => {
                let item = Item::new(ItemKind::Image(ImageItem { image, transform: gs.ctm, attributes: BTreeMap::new() }));
                self.emit(item, mc);
            }
            Err(e) => self.out.warn(format!("An image could not be read: {e}")),
        }
    }
}

/// Makes sure a segment has a part to go into: a new one at the origin when nothing was drawn
/// yet, a new one at the current point when the last part was closed (`h` or `re`).
fn ensure_start(pb: &mut PathBuilder) {
    match pb.current {
        None => {
            pb.path.move_to(Point::ZERO);
            pb.current = Some(Point::ZERO);
            pb.start = pb.current;
        }
        Some(p) if pb.reopen => {
            pb.path.move_to(p);
            pb.start = Some(p);
        }
        Some(_) => {}
    }
    pb.reopen = false;
}

fn inherited<'a>(doc: &'a Document, page: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut d = page;
    for _ in 0..32 {
        if let Some(v) = dict_get(doc, d, key) {
            return Some(v);
        }
        d = dict_get(doc, d, b"Parent")?.as_dict().ok()?;
    }
    None
}

/// Decodes a PDF text string (UTF-16BE with BOM, or PDFDocEncoding approximated as Latin-1).
fn pdf_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let units: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    b.iter().map(|&c| c as char).collect()
}

fn colour_space(doc: &Document, o: &Object, res: &Res<'_>, depth: u32) -> Cs {
    if depth > 8 {
        return Cs::Gray;
    }
    match o {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Cs::Gray,
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Cs::Rgb,
            b"DeviceCMYK" | b"CMYK" => Cs::Cmyk,
            b"Pattern" => Cs::Pattern,
            other => match res.lookup(doc, b"ColorSpace", other) {
                Some((o, _)) if !matches!(o, Object::Name(x) if x.as_slice() == other) => colour_space(doc, o, res, depth + 1),
                _ => Cs::Gray,
            },
        },
        Object::Array(a) => {
            let kind = a.first().and_then(name).unwrap_or(b"");
            match kind {
                b"ICCBased" => {
                    let s = a.get(1).map(|o| deref(doc, o)).and_then(|o| o.as_stream().ok());
                    let n = s.and_then(|s| s.dict.get(b"N").ok()).and_then(num).unwrap_or(3.0) as usize;
                    match n {
                        1 => Cs::Gray,
                        4 => Cs::Cmyk,
                        _ => Cs::Rgb,
                    }
                }
                b"CalGray" => Cs::Gray,
                b"CalRGB" => Cs::Rgb,
                b"Lab" => Cs::Lab,
                b"Indexed" | b"I" => {
                    let base = a.get(1).map(|o| colour_space(doc, deref(doc, o), res, depth + 1)).unwrap_or(Cs::Rgb);
                    let hival = a.get(2).and_then(num).unwrap_or(0.0).clamp(0.0, 255.0) as usize;
                    let lookup = match a.get(3).map(|o| deref(doc, o)) {
                        Some(Object::String(s, _)) => s.clone(),
                        Some(Object::Stream(s)) => s.decompressed_content_with_limit(1 << 20).unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    Cs::Indexed { base: Box::new(base), hival, lookup }
                }
                b"Separation" => Cs::Tint(1),
                b"DeviceN" => {
                    let n = a.get(1).map(|o| deref(doc, o)).and_then(|o| o.as_array().ok()).map(|v| v.len()).unwrap_or(1);
                    Cs::Tint(n.max(1))
                }
                b"Pattern" => Cs::Pattern,
                other => colour_space(doc, &Object::Name(other.to_vec()), res, depth + 1),
            }
        }
        Object::Reference(_) => colour_space(doc, deref(doc, o), res, depth + 1),
        _ => Cs::Gray,
    }
}

fn get_abbr<'a>(doc: &'a Document, d: &'a Dictionary, abbr: &[u8], full: &[u8]) -> Option<&'a Object> {
    dict_get(doc, d, full).or_else(|| dict_get(doc, d, abbr))
}

fn filter_names(doc: &Document, d: &Dictionary) -> Vec<Vec<u8>> {
    match get_abbr(doc, d, b"F", b"Filter") {
        Some(Object::Name(n)) => vec![n.clone()],
        Some(Object::Array(a)) => a.iter().filter_map(|o| deref(doc, o).as_name().ok().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    }
}

fn expand_filter(n: &[u8]) -> &[u8] {
    match n {
        b"AHx" => b"ASCIIHexDecode",
        b"A85" => b"ASCII85Decode",
        b"LZW" => b"LZWDecode",
        b"Fl" => b"FlateDecode",
        b"RL" => b"RunLengthDecode",
        b"CCF" => b"CCITTFaxDecode",
        b"DCT" => b"DCTDecode",
        other => other,
    }
}

/// Decodes an image XObject or inline image into an RGB(A) or gray image.
fn decode_image(doc: &Document, dict: &Dictionary, raw: &[u8], fill: Rgb, res: &Res<'_>, inline: bool) -> Result<DynamicImage, String> {
    let width = get_abbr(doc, dict, b"W", b"Width").and_then(num).unwrap_or(0.0);
    let height = get_abbr(doc, dict, b"H", b"Height").and_then(num).unwrap_or(0.0);
    if !(width >= 1.0 && height >= 1.0) || width * height > MAX_PIXELS as f64 {
        return Err("bad image size".into());
    }
    let (w, h) = (width as u32, height as u32);
    let is_mask = get_abbr(doc, dict, b"IM", b"ImageMask").and_then(|o| o.as_bool().ok()).unwrap_or(false);
    let filters: Vec<Vec<u8>> = filter_names(doc, dict).iter().map(|f| expand_filter(f).to_vec()).collect();
    let last = filters.last().map(Vec::as_slice).unwrap_or(b"");

    // Bytes after all "plain" filters.
    let plain_stage = |dict: &Dictionary, raw: &[u8], upto: usize| -> Result<Vec<u8>, String> {
        if upto == 0 || inline {
            return Ok(raw.to_vec());
        }
        let mut d = dict.clone();
        let kept: Vec<Object> = filters[..upto].iter().map(|f| Object::Name(f.clone())).collect();
        d.set("Filter", Object::Array(kept));
        let s = lopdf::Stream::new(d, raw.to_vec());
        s.decompressed_content_with_limit(MAX_DECODED).map_err(|e| e.to_string())
    };

    let mut img: DynamicImage = match last {
        b"DCTDecode" => {
            let bytes = plain_stage(dict, raw, filters.len() - 1)?;
            guard("JPEG", || image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg))
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
        }
        b"JPXDecode" => return Err("JPEG 2000 images are not supported".into()),
        b"JBIG2Decode" => return Err("JBIG2 images are not supported".into()),
        b"CCITTFaxDecode" => {
            let bytes = plain_stage(dict, raw, filters.len() - 1)?;
            let parms = get_abbr(doc, dict, b"DP", b"DecodeParms").map(|o| match o {
                Object::Array(a) => a.last().map(|x| deref(doc, x)).unwrap_or(o),
                _ => o,
            });
            let pd = parms.and_then(|o| o.as_dict().ok());
            let k = pd.and_then(|d| dict_get(doc, d, b"K")).and_then(num).unwrap_or(0.0);
            let cols = pd.and_then(|d| dict_get(doc, d, b"Columns")).and_then(num).unwrap_or(1728.0) as u32;
            let black_is_1 = pd.and_then(|d| dict_get(doc, d, b"BlackIs1")).and_then(|o| o.as_bool().ok()).unwrap_or(false);
            let samples = decode_ccitt(&bytes, k, cols.max(1), h, black_is_1)?;
            samples_to_image(&samples, cols.max(1), h, 1, &Cs::Gray, is_mask, fill, decode_array(doc, dict), true)?
        }
        _ => {
            let data = if inline { raw.to_vec() } else { plain_stage(dict, raw, filters.len())? };
            let bpc = if is_mask { 1 } else { get_abbr(doc, dict, b"BPC", b"BitsPerComponent").and_then(num).unwrap_or(8.0) as u32 };
            if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
                return Err(format!("unsupported bit depth {bpc}"));
            }
            let cs = if is_mask {
                Cs::Gray
            } else {
                match get_abbr(doc, dict, b"CS", b"ColorSpace") {
                    Some(o) => colour_space(doc, o, res, 0),
                    None => Cs::Gray,
                }
            };
            samples_to_image(&data, w, h, bpc, &cs, is_mask, fill, decode_array(doc, dict), false)?
        }
    };
    // Soft mask -> alpha.
    if !inline
        && let Some(sm) = dict_get(doc, dict, b"SMask").and_then(|o| o.as_stream().ok())
        && let Ok(mask) = decode_image(doc, &sm.dict, &sm.content, Rgb::BLACK, res, false)
    {
        let mask = mask.to_luma8();
        let mut rgba = img.to_rgba8();
        if mask.dimensions() == rgba.dimensions() {
            for (p, m) in rgba.pixels_mut().zip(mask.pixels()) {
                p.0[3] = m.0[0];
            }
            img = DynamicImage::ImageRgba8(rgba);
        }
    }
    Ok(img)
}

fn decode_array(doc: &Document, dict: &Dictionary) -> Option<Vec<f64>> {
    get_abbr(doc, dict, b"D", b"Decode").and_then(|o| o.as_array().ok()).map(|a| nums(a))
}

/// CCITT G3/G4 to one byte per pixel (0 = black, 1 = white, as DeviceGray 1-bit samples).
fn decode_ccitt(data: &[u8], k: f64, cols: u32, rows: u32, black_is_1: bool) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity((cols as usize) * (rows as usize));
    let mut lines = 0u32;
    let mut push_line = |transitions: &[u32]| {
        if lines >= rows {
            return;
        }
        lines += 1;
        for c in fax::decoder::pels(transitions, cols) {
            let black = c == fax::Color::Black;
            // Sample value as stored: with BlackIs1 a black pixel is 1.
            out.push(if black == black_is_1 { 1 } else { 0 });
        }
    };
    let ok = if k < 0.0 {
        guard("CCITT", || fax::decoder::decode_g4(data.iter().copied(), cols, Some(rows), &mut push_line)).ok().flatten()
    } else {
        guard("CCITT", || fax::decoder::decode_g3(data.iter().copied(), &mut push_line)).ok().flatten()
    };
    if ok.is_none() && out.is_empty() {
        return Err("CCITT data could not be decoded".into());
    }
    out.resize((cols as usize) * (rows as usize), if black_is_1 { 0 } else { 1 });
    Ok(out)
}

/// Turns raw samples into an image. `unpacked` means one byte per sample (CCITT output).
fn samples_to_image(
    data: &[u8],
    w: u32,
    h: u32,
    bpc: u32,
    cs: &Cs,
    is_mask: bool,
    fill: Rgb,
    decode: Option<Vec<f64>>,
    unpacked: bool,
) -> Result<DynamicImage, String> {
    let ncomp = cs.components();
    let row_bits = (w as usize) * ncomp * bpc as usize;
    let row_bytes = if unpacked { (w as usize) * ncomp } else { row_bits.div_ceil(8) };
    let needed = row_bytes * h as usize;
    if data.len() < needed / 2 {
        return Err("image data is too short".into());
    }
    let max = ((1u32 << bpc.min(16)) - 1) as f64;
    let sample = |row: usize, idx: usize| -> u32 {
        let base = row * row_bytes;
        if unpacked {
            return u32::from(*data.get(base + idx).unwrap_or(&0));
        }
        match bpc {
            8 => u32::from(*data.get(base + idx).unwrap_or(&0)),
            16 => {
                let i = base + idx * 2;
                (u32::from(*data.get(i).unwrap_or(&0)) << 8) | u32::from(*data.get(i + 1).unwrap_or(&0))
            }
            _ => {
                let bit = idx * bpc as usize;
                let byte = *data.get(base + bit / 8).unwrap_or(&0);
                let shift = 8 - bpc as usize - (bit % 8);
                (u32::from(byte) >> shift) & ((1 << bpc) - 1)
            }
        }
    };
    let indexed_max = match cs {
        Cs::Indexed { .. } => max,
        _ => 1.0,
    };
    // Decode range per component.
    let ranges: Vec<(f64, f64)> = (0..ncomp)
        .map(|c| match &decode {
            Some(d) if d.len() >= 2 * (c + 1) => (d[2 * c], d[2 * c + 1]),
            _ => (0.0, indexed_max),
        })
        .collect();
    if is_mask {
        // Stencil mask: sample 0 paints (unless Decode is [1 0]).
        let invert = decode.as_ref().is_some_and(|d| d.len() >= 2 && d[0] > d[1]);
        let mut img = RgbaImage::new(w, h);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let s = sample(y, x);
                let paint = (s == 0) != invert;
                img.put_pixel(x as u32, y as u32, image::Rgba([fill.0, fill.1, fill.2, if paint { 255 } else { 0 }]));
            }
        }
        return Ok(DynamicImage::ImageRgba8(img));
    }
    if matches!(cs, Cs::Gray) && ncomp == 1 {
        let mut img = GrayImage::new(w, h);
        let (d0, d1) = ranges[0];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let v = d0 + f64::from(sample(y, x)) * (d1 - d0) / max;
                img.put_pixel(x as u32, y as u32, image::Luma([(v.clamp(0.0, 1.0) * 255.0).round() as u8]));
            }
        }
        return Ok(DynamicImage::ImageLuma8(img));
    }
    let mut img = RgbImage::new(w, h);
    let mut comps = vec![0.0; ncomp];
    for y in 0..h as usize {
        for x in 0..w as usize {
            for (c, slot) in comps.iter_mut().enumerate() {
                let (d0, d1) = ranges[c];
                *slot = d0 + f64::from(sample(y, x * ncomp + c)) * (d1 - d0) / max;
            }
            let rgb = cs.to_rgb(&comps);
            img.put_pixel(x as u32, y as u32, image::Rgb([rgb.0, rgb.1, rgb.2]));
        }
    }
    Ok(DynamicImage::ImageRgb8(img))
}
