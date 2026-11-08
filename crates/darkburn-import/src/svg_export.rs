//! SVG export: paths with colours, dash patterns, embedded images (base64 PNG), text, and one
//! Inkscape-style layer group per item layer. Units are millimetres.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use kurbo::{Affine, BezPath, PathEl, Rect};

use crate::geom::fmt_num;
use crate::model::*;
use crate::xml::escape;

/// Where the SVG's origin sits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SvgOrigin {
    /// The page is the bounding box of the exported items.
    #[default]
    Bounds,
    /// A fixed page `width` x `height` mm with the drawing origin at its bottom-left corner.
    Page { width: f64, height: f64 },
}

#[derive(Clone, Debug)]
pub struct SvgExportOptions {
    pub origin: SvgOrigin,
    /// Decimals for coordinates.
    pub decimals: usize,
    /// Stroke width in mm used for drawing.
    pub stroke_width: f64,
}

impl Default for SvgExportOptions {
    fn default() -> Self {
        SvgExportOptions { origin: SvgOrigin::Bounds, decimals: 4, stroke_width: 0.1 }
    }
}

/// SVG path data for a path.
pub fn path_data(path: &BezPath, decimals: usize) -> String {
    let f = |v: f64| fmt_num(v, decimals);
    let mut d = String::new();
    for el in path.elements() {
        if !d.is_empty() {
            d.push(' ');
        }
        match el {
            PathEl::MoveTo(p) => d.push_str(&format!("M{} {}", f(p.x), f(p.y))),
            PathEl::LineTo(p) => d.push_str(&format!("L{} {}", f(p.x), f(p.y))),
            PathEl::QuadTo(a, p) => d.push_str(&format!("Q{} {} {} {}", f(a.x), f(a.y), f(p.x), f(p.y))),
            PathEl::CurveTo(a, b, p) => d.push_str(&format!("C{} {} {} {} {} {}", f(a.x), f(a.y), f(b.x), f(b.y), f(p.x), f(p.y))),
            PathEl::ClosePath => d.push('Z'),
        }
    }
    d
}

fn matrix(a: Affine, decimals: usize) -> String {
    let c = a.as_coeffs();
    format!("matrix({})", c.iter().map(|v| fmt_num(*v, decimals.max(6))).collect::<Vec<_>>().join(" "))
}

fn colour(c: Option<Rgb>) -> String {
    match c {
        Some(c) => format!("#{}", c.hex().to_ascii_lowercase()),
        None => "none".into(),
    }
}

/// Writes items as an SVG document.
pub fn export(items: &[Item], opts: &SvgExportOptions) -> String {
    let visible: Vec<&Item> = items.iter().filter(|i| !i.hidden).collect();
    let page = match opts.origin {
        SvgOrigin::Bounds => bounds_of(visible.iter().copied()).unwrap_or(Rect::ZERO),
        SvgOrigin::Page { width, height } => Rect::new(0.0, 0.0, width, height),
    };
    let (w, h) = (page.width().max(0.0), page.height().max(0.0));
    // Drawing (Y up) to SVG (Y down).
    let flip = Affine::new([1.0, 0.0, 0.0, -1.0, -page.x0, page.y1]);
    let dec = opts.decimals;
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
         xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" version=\"1.1\" \
         width=\"{}mm\" height=\"{}mm\" viewBox=\"0 0 {} {}\">\n",
        fmt_num(w, dec),
        fmt_num(h, dec),
        fmt_num(w, dec),
        fmt_num(h, dec)
    ));
    // Layers in order of first use.
    let mut order: Vec<String> = Vec::new();
    let mut by_layer: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for item in &visible {
        let key = item.layer.clone().unwrap_or_else(|| item.colour().map(|c| format!("#{}", c.hex())).unwrap_or_else(|| "Layer".into()));
        if !by_layer.contains_key(&key) {
            order.push(key.clone());
        }
        by_layer.entry(key).or_default().push(item);
    }
    for (li, key) in order.iter().enumerate() {
        s.push_str(&format!("  <g id=\"layer{}\" inkscape:groupmode=\"layer\" inkscape:label=\"{}\">\n", li + 1, escape(key)));
        for item in &by_layer[key] {
            write_item(&mut s, item, flip, opts);
        }
        s.push_str("  </g>\n");
    }
    s.push_str("</svg>\n");
    s
}

fn write_item(s: &mut String, item: &Item, flip: Affine, opts: &SvgExportOptions) {
    let dec = opts.decimals;
    let sw = fmt_num(opts.stroke_width, 4);
    match &item.kind {
        ItemKind::Path(p) => {
            let mut path = p.path.clone();
            path.apply_affine(flip);
            let mut style = format!("fill:{};stroke:{}", colour(p.fill), colour(p.stroke));
            if p.stroke.is_some() {
                style.push_str(&format!(";stroke-width:{sw}"));
            }
            if let Some(d) = &p.dash {
                let arr: Vec<String> = d.iter().map(|v| fmt_num(*v, 4)).collect();
                style.push_str(&format!(";stroke-dasharray:{}", arr.join(",")));
            }
            s.push_str(&format!("    <path style=\"{style}\" d=\"{}\"/>\n", path_data(&path, dec)));
        }
        ItemKind::Text(t) => {
            if let Some(o) = &t.outline {
                let mut path = o.clone();
                path.apply_affine(flip);
                s.push_str(&format!(
                    "    <path style=\"fill:{};stroke:none\" d=\"{}\"><title>{}</title></path>\n",
                    colour(Some(t.colour)),
                    path_data(&path, dec),
                    escape(&t.text)
                ));
            } else {
                let m = flip * t.transform * Affine::scale_non_uniform(1.0, -1.0);
                let anchor = match t.h_align {
                    HAlign::Left => "start",
                    HAlign::Center => "middle",
                    HAlign::Right => "end",
                };
                s.push_str(&format!(
                    "    <text transform=\"{}\" style=\"font-family:{};font-size:{}px;fill:{};text-anchor:{anchor}\">{}</text>\n",
                    matrix(m, dec),
                    escape(&t.font),
                    fmt_num(t.height, 4),
                    colour(Some(t.colour)),
                    escape(&t.text)
                ));
            }
        }
        ItemKind::Image(img) => {
            let mut png = Vec::new();
            if img.image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
                return;
            }
            // SVG image space (0,0 top-left, 1x1) to the unit square with v up.
            let m = flip * img.transform * Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]);
            s.push_str(&format!(
                "    <image width=\"1\" height=\"1\" preserveAspectRatio=\"none\" transform=\"{}\" xlink:href=\"data:image/png;base64,{}\"/>\n",
                matrix(m, dec),
                B64.encode(&png)
            ));
        }
    }
}
