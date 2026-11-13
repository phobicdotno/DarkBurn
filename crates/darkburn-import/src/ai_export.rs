//! Adobe Illustrator export as a PDF-compatible `.ai` file (also a valid `.pdf`).
//!
//! Illustrator opens PDF-compatible AI files through their PDF part, so this writes a one-page
//! PDF: vector paths with stroke and fill colours, images (Flate RGB with soft-mask alpha),
//! text (outline when available, else Helvetica) and one optional-content layer per item layer,
//! which Illustrator and Acrobat show as layers.

use std::collections::BTreeMap;
use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use kurbo::{Affine, BezPath, PathEl, Point, Rect};
use lopdf::{Dictionary, Document, Object, Stream, StringFormat};

use crate::geom::fmt_num;
use crate::model::*;

const MM_TO_PT: f64 = 72.0 / 25.4;

/// The written file plus anything that could not be written.
#[derive(Clone, Debug, Default)]
pub struct PdfExport {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AiExportOptions {
    /// Stroke width in mm.
    pub stroke_width: f64,
    /// Margin around the artwork in mm.
    pub margin: f64,
}

impl Default for AiExportOptions {
    fn default() -> Self {
        AiExportOptions { stroke_width: 0.1, margin: 0.0 }
    }
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

fn n(v: f64) -> String {
    fmt_num(v, 4)
}

fn path_ops(path: &BezPath, s: &mut String) {
    let mut last = Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                s.push_str(&format!("{} {} m\n", n(p.x), n(p.y)));
                last = p;
            }
            PathEl::LineTo(p) => {
                s.push_str(&format!("{} {} l\n", n(p.x), n(p.y)));
                last = p;
            }
            PathEl::QuadTo(q, p) => {
                let c0 = last + (q - last) * (2.0 / 3.0);
                let c1 = p + (q - p) * (2.0 / 3.0);
                s.push_str(&format!("{} {} {} {} {} {} c\n", n(c0.x), n(c0.y), n(c1.x), n(c1.y), n(p.x), n(p.y)));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                s.push_str(&format!("{} {} {} {} {} {} c\n", n(a.x), n(a.y), n(b.x), n(b.y), n(p.x), n(p.y)));
                last = p;
            }
            PathEl::ClosePath => s.push_str("h\n"),
        }
    }
}

/// A PDF real; PDF has no spelling for NaN or the infinities.
fn real(v: f64) -> Object {
    Object::Real(if v.is_nan() { 0.0 } else { v.clamp(f64::from(f32::MIN), f64::from(f32::MAX)) as f32 })
}

fn rgb_op(c: Rgb, op: &str) -> String {
    format!("{} {} {} {op}\n", n(f64::from(c.0) / 255.0), n(f64::from(c.1) / 255.0), n(f64::from(c.2) / 255.0))
}

fn cm(a: Affine) -> String {
    let c = a.as_coeffs();
    format!("{} {} {} {} {} {}", fmt_num(c[0], 6), fmt_num(c[1], 6), fmt_num(c[2], 6), fmt_num(c[3], 6), fmt_num(c[4], 4), fmt_num(c[5], 4))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
}

fn escape_pdf_string(b: &[u8]) -> String {
    let mut out = String::new();
    for &c in b {
        match c {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(c as char);
            }
            32..=126 => out.push(c as char),
            _ => out.push_str(&format!("\\{c:03o}")),
        }
    }
    out
}

/// Writes items as a PDF-compatible Adobe Illustrator file (or a plain PDF).
pub fn export(items: &[Item], opts: &AiExportOptions) -> PdfExport {
    let mut warnings = Vec::new();
    let visible: Vec<&Item> = items.iter().filter(|i| !i.hidden).collect();
    let b = bounds_of(visible.iter().copied()).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0)).inflate(opts.margin, opts.margin);
    let b = if b.width() <= 0.0 || b.height() <= 0.0 { Rect::new(b.x0, b.y0, b.x0 + 1.0, b.y0 + 1.0) } else { b };
    let to_pt = Affine::scale(MM_TO_PT) * Affine::translate((-b.x0, -b.y0));
    let (w, h) = (b.width() * MM_TO_PT, b.height() * MM_TO_PT);

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    // Layers in order of first use.
    let mut layer_order: Vec<String> = Vec::new();
    for i in &visible {
        let l = i.layer.clone().unwrap_or_else(|| "Layer 1".into());
        if !layer_order.contains(&l) {
            layer_order.push(l);
        }
    }
    let mut ocgs: BTreeMap<String, (String, lopdf::ObjectId)> = BTreeMap::new();
    let mut properties = Dictionary::new();
    for (k, name) in layer_order.iter().enumerate() {
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"OCG".to_vec()));
        d.set("Name", Object::String(latin1(name), StringFormat::Literal));
        let id = doc.add_object(d);
        let tag = format!("L{k}");
        properties.set(tag.clone(), Object::Reference(id));
        ocgs.insert(name.clone(), (tag, id));
    }

    let mut content = String::new();
    content.push_str(&format!("{} w\n1 J\n1 j\n", n(opts.stroke_width * MM_TO_PT)));
    let mut xobjects = Dictionary::new();
    let mut used_font = false;
    let mut img_count = 0;
    for layer in &layer_order {
        let (tag, _) = &ocgs[layer];
        content.push_str(&format!("/OC /{tag} BDC\n"));
        for item in visible.iter().filter(|i| i.layer.clone().unwrap_or_else(|| "Layer 1".into()) == *layer) {
            match &item.kind {
                ItemKind::Path(p) => {
                    if p.stroke.is_none() && p.fill.is_none() {
                        continue;
                    }
                    let mut path = p.path.clone();
                    path.apply_affine(to_pt);
                    if let Some(c) = p.stroke {
                        content.push_str(&rgb_op(c, "RG"));
                    }
                    if let Some(c) = p.fill {
                        content.push_str(&rgb_op(c, "rg"));
                    }
                    path_ops(&path, &mut content);
                    content.push_str(match (p.stroke.is_some(), p.fill.is_some()) {
                        (true, true) => "B\n",
                        (false, true) => "f\n",
                        _ => "S\n",
                    });
                }
                ItemKind::Text(t) => {
                    content.push_str(&rgb_op(t.colour, "rg"));
                    if let Some(o) = &t.outline {
                        let mut path = o.clone();
                        path.apply_affine(to_pt);
                        path_ops(&path, &mut content);
                        content.push_str("f\n");
                    } else {
                        used_font = true;
                        let m = to_pt * t.transform;
                        for (li, line) in t.text.lines().enumerate() {
                            let lm = m * Affine::translate((0.0, -(li as f64) * t.height * 1.2));
                            content.push_str(&format!("BT\n/F1 {} Tf\n{} Tm\n({}) Tj\nET\n", n(t.height), cm(lm), escape_pdf_string(&latin1(line))));
                        }
                    }
                }
                ItemKind::Image(img) => {
                    let rgba = img.image.to_rgba8();
                    let (iw, ih) = rgba.dimensions();
                    if iw == 0 || ih == 0 {
                        continue;
                    }
                    let mut rgb = Vec::with_capacity((iw * ih * 3) as usize);
                    let mut alpha = Vec::with_capacity((iw * ih) as usize);
                    for p in rgba.pixels() {
                        rgb.extend_from_slice(&p.0[..3]);
                        alpha.push(p.0[3]);
                    }
                    let mut d = Dictionary::new();
                    d.set("Type", Object::Name(b"XObject".to_vec()));
                    d.set("Subtype", Object::Name(b"Image".to_vec()));
                    d.set("Width", Object::Integer(i64::from(iw)));
                    d.set("Height", Object::Integer(i64::from(ih)));
                    d.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
                    d.set("BitsPerComponent", Object::Integer(8));
                    d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
                    if alpha.iter().any(|a| *a != 255) {
                        let mut md = Dictionary::new();
                        md.set("Type", Object::Name(b"XObject".to_vec()));
                        md.set("Subtype", Object::Name(b"Image".to_vec()));
                        md.set("Width", Object::Integer(i64::from(iw)));
                        md.set("Height", Object::Integer(i64::from(ih)));
                        md.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
                        md.set("BitsPerComponent", Object::Integer(8));
                        md.set("Filter", Object::Name(b"FlateDecode".to_vec()));
                        let mid = doc.add_object(Stream::new(md, zlib(&alpha)));
                        d.set("SMask", Object::Reference(mid));
                    }
                    let id = doc.add_object(Stream::new(d, zlib(&rgb)));
                    let tag = format!("Im{img_count}");
                    img_count += 1;
                    xobjects.set(tag.clone(), Object::Reference(id));
                    content.push_str(&format!("q\n{} cm\n/{tag} Do\nQ\n", cm(to_pt * img.transform)));
                }
            }
        }
        content.push_str("EMC\n");
    }
    if used_font {
        warnings.push("Text without an outline was written with a standard font".into());
    }

    let mut resources = Dictionary::new();
    if !xobjects.is_empty() {
        resources.set("XObject", Object::Dictionary(xobjects));
    }
    if !properties.is_empty() {
        resources.set("Properties", Object::Dictionary(properties));
    }
    if used_font {
        let mut f = Dictionary::new();
        f.set("Type", Object::Name(b"Font".to_vec()));
        f.set("Subtype", Object::Name(b"Type1".to_vec()));
        f.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        f.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let fid = doc.add_object(f);
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Reference(fid));
        resources.set("Font", Object::Dictionary(fonts));
    }
    let mut cd = Dictionary::new();
    cd.set("Filter", Object::Name(b"FlateDecode".to_vec()));
    let content_id = doc.add_object(Stream::new(cd, zlib(content.as_bytes())));
    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set("MediaBox", Object::Array(vec![0.into(), 0.into(), real(w), real(h)]));
    page.set("Contents", Object::Reference(content_id));
    page.set("Resources", Object::Dictionary(resources));
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    if !ocgs.is_empty() {
        let refs: Vec<Object> = layer_order.iter().map(|l| Object::Reference(ocgs[l].1)).collect();
        let mut d = Dictionary::new();
        d.set("Order", Object::Array(refs.clone()));
        d.set("ON", Object::Array(refs.clone()));
        let mut ocp = Dictionary::new();
        ocp.set("OCGs", Object::Array(refs));
        ocp.set("D", Object::Dictionary(d));
        catalog.set("OCProperties", Object::Dictionary(ocp));
    }
    let catalog_id = doc.add_object(catalog);
    let mut info = Dictionary::new();
    info.set("Creator", Object::String(b"DarkBurn".to_vec(), StringFormat::Literal));
    info.set("Producer", Object::String(b"DarkBurn".to_vec(), StringFormat::Literal));
    let info_id = doc.add_object(info);
    doc.trailer.set("Root", Object::Reference(catalog_id));
    doc.trailer.set("Info", Object::Reference(info_id));

    let mut bytes = Vec::new();
    if let Err(e) = doc.save_to(&mut bytes) {
        warnings.push(format!("PDF writer failed: {e}"));
    }
    PdfExport { bytes, warnings }
}
