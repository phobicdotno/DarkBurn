//! LightBurn project files: `.lbrn` (FormatVersion 0, verbose XML) and `.lbrn2`
//! (FormatVersion 1, compact vertex/primitive strings). Reader and `.lbrn2` writer.
//!
//! Written from the public format notes in DarkBurn's research report and the MIT-licensed
//! readers in MeerK40t (`meerk40t/extra/lbrn.py`) and lbrn2-to-svg; see `ATTRIBUTION.md`.
//!
//! Shape coordinates are millimetres with Y up. Each shape carries `<XForm>a b c d e f</XForm>`
//! (`x' = a*x + c*y + e`, `y' = b*x + d*y + f`); groups compose with their children.

use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use kurbo::{Affine, BezPath, Ellipse, PathEl, Point, Rect, RoundedRect, Shape};

use crate::geom::{self, fmt_num};
use crate::model::*;
use crate::xml::{self, Element};
use crate::{ImportError, guard};

#[derive(Clone, Copy, Debug, Default)]
struct Vert {
    p: Point,
    c0: Option<Point>,
    c1: Option<Point>,
}

#[derive(Clone, Copy, Debug)]
enum Prim {
    Line(usize, usize),
    Bez(usize, usize),
}

#[derive(Clone, Debug)]
enum PrimSpec {
    List(Vec<Prim>),
    LineClosed,
    AutoClosed,
}

struct Reader {
    out: Imported,
    verts: HashMap<String, Vec<Vert>>,
    prims: HashMap<String, PrimSpec>,
    names: HashMap<u32, String>,
    budget: usize,
}

/// Imports a LightBurn project.
pub fn import(bytes: &[u8]) -> Result<Imported, ImportError> {
    let text = String::from_utf8_lossy(bytes);
    let root = xml::parse(&text).map_err(|e| ImportError::Parse(format!("LightBurn project: {e}")))?;
    if root.name != "LightBurnProject" {
        return Err(ImportError::Parse("not a LightBurn project".into()));
    }
    let mut r = Reader { out: Imported::default(), verts: HashMap::new(), prims: HashMap::new(), names: HashMap::new(), budget: 2_000_000 };

    let mut info = ProjectInfo {
        app_version: root.attr("AppVersion").map(str::to_string),
        format_version: root.attr("FormatVersion").and_then(|v| v.trim().parse().ok()),
        material_height: root.attr_f64("MaterialHeight"),
        mirror_x: truthy(root.attr("MirrorX")),
        mirror_y: truthy(root.attr("MirrorY")),
        ..Default::default()
    };
    for c in &root.children {
        match c.name.as_str() {
            "Thumbnail" => info.thumbnail_png = c.attr("Source").and_then(|s| B64.decode(s.trim()).ok()),
            "VariableText" => info.variable_text = value_children(c),
            "UIPrefs" => info.ui_prefs = value_children(c),
            "Notes" => {
                info.show_notes_on_load = truthy(c.attr("ShowOnLoad"));
                info.notes = c.attr("Notes").map(str::to_string).filter(|n| !n.is_empty());
            }
            "CutSetting" | "CutSetting_Img" => {
                let ls = cut_setting(c);
                r.names.entry(ls.index).or_insert_with(|| ls.name.clone());
                r.out.layer_settings.push(ls);
            }
            _ => {}
        }
    }
    for c in root.children.iter().filter(|c| c.name == "Shape") {
        r.shape(c, Affine::IDENTITY, None, 0);
    }
    r.out.project = Some(info);
    Ok(crate::finish(r.out))
}

fn truthy(v: Option<&str>) -> bool {
    matches!(v.map(|s| s.trim().to_ascii_lowercase()).as_deref(), Some("true" | "1" | "yes"))
}

fn value_children(e: &Element) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = e.attrs.iter().cloned().collect();
    for c in &e.children {
        if let Some(v) = c.attr("Value") {
            m.insert(c.name.clone(), v.to_string());
        }
    }
    m
}

fn cut_setting(e: &Element) -> LayerSettings {
    let mut values = value_children(e);
    let mode = values.remove("type").unwrap_or_else(|| if e.name == "CutSetting_Img" { "Image".into() } else { "Cut".into() });
    if e.name == "CutSetting_Img" {
        values.insert("tag".into(), e.name.clone());
    }
    let index = values.get("index").and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v.max(0.0) as u32).unwrap_or(0);
    let name = values.get("name").cloned().unwrap_or_else(|| format!("C{index:02}"));
    LayerSettings { index, name, mode, values }
}

fn parse_xform(e: &Element) -> Affine {
    let Some(x) = e.child("XForm") else { return Affine::IDENTITY };
    let v: Vec<f64> = x.text.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect();
    if v.len() == 6 && v.iter().all(|f| f.is_finite()) { Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]) } else { Affine::IDENTITY }
}

impl Reader {
    fn push(&mut self, item: Item) {
        if self.budget > 0 {
            self.budget -= 1;
            self.out.items.push(item);
        }
    }

    fn layer_of(&self, e: &Element) -> (Option<String>, Option<u32>) {
        let idx = e.attr_f64("CutIndex").map(|v| v.max(0.0) as u32);
        let name = idx.map(|i| self.names.get(&i).cloned().unwrap_or_else(|| format!("C{i:02}")));
        (name, idx)
    }

    fn shape(&mut self, e: &Element, parent: Affine, group: Option<u32>, depth: u32) {
        if depth > 64 {
            self.out.warn("Groups nested too deeply were cut off");
            return;
        }
        let xf = parent * parse_xform(e);
        let (layer, layer_index) = self.layer_of(e);
        let ty = e.attr("Type").unwrap_or("");
        let make = |kind: ItemKind| {
            let mut it = Item::new(kind);
            it.layer = layer.clone();
            it.layer_index = layer_index;
            it.group = group;
            it.hidden = truthy(e.attr("Hidden"));
            it
        };
        match ty {
            "Rect" => {
                let w = e.attr_f64("W").unwrap_or(0.0);
                let h = e.attr_f64("H").unwrap_or(0.0);
                let cr = e.attr_f64("Cr").unwrap_or(0.0).clamp(0.0, w.abs().min(h.abs()) / 2.0);
                let rect = Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
                let mut p = if cr > 0.0 { RoundedRect::from_rect(rect, cr).to_path(geom::ARC_TOLERANCE) } else { rect.to_path(0.0) };
                p.apply_affine(xf);
                self.push(make(path_kind(p)));
            }
            "Ellipse" => {
                let rx = e.attr_f64("Rx").unwrap_or(0.0).abs();
                let ry = e.attr_f64("Ry").unwrap_or(0.0).abs();
                let mut p = Ellipse::new(Point::ZERO, (rx, ry), 0.0).to_path(geom::ARC_TOLERANCE);
                p.apply_affine(xf);
                self.push(make(path_kind(p)));
            }
            "Polygon" => {
                let n = e.attr_f64("N").unwrap_or(0.0).clamp(0.0, 10_000.0) as usize;
                let rx = e.attr_f64("Rx").unwrap_or(0.0);
                let ry = e.attr_f64("Ry").unwrap_or(rx);
                if n >= 3 {
                    let mut p = BezPath::new();
                    for k in 0..n {
                        let a = std::f64::consts::TAU * k as f64 / n as f64;
                        let pt = Point::new(rx * a.cos(), ry * a.sin());
                        if k == 0 { p.move_to(pt) } else { p.line_to(pt) }
                    }
                    p.close_path();
                    p.apply_affine(xf);
                    self.push(make(path_kind(p)));
                } else if e.child("VertList").is_some() || e.attr("VertID").is_some() {
                    let p = self.path_geometry(e, xf);
                    self.push(make(path_kind(p)));
                }
            }
            "Path" => {
                let p = self.path_geometry(e, xf);
                if !p.is_empty() {
                    self.push(make(path_kind(p)));
                }
            }
            "Text" => {
                let outline = e.children_named("BackupPath").next().map(|b| {
                    let bxf = xf * parse_xform(b);
                    self.path_geometry(b, bxf)
                });
                let mut attributes: BTreeMap<String, String> = BTreeMap::new();
                for (k, v) in &e.attrs {
                    if !matches!(k.as_str(), "Type" | "Str" | "Font" | "H" | "CutIndex") {
                        attributes.insert(k.clone(), v.clone());
                    }
                }
                let h_align = match e.attr_f64("Ah").unwrap_or(0.0) as i32 {
                    1 => HAlign::Center,
                    2 => HAlign::Right,
                    _ => HAlign::Left,
                };
                let text = TextItem {
                    text: e.attr("Str").unwrap_or("").to_string(),
                    font: e.attr("Font").map(|f| f.split(',').next().unwrap_or(f).to_string()).unwrap_or_default(),
                    style: e.attr("Font").map(str::to_string),
                    height: e.attr_f64("H").unwrap_or(0.0),
                    h_align,
                    v_align: VAlign::Baseline,
                    transform: xf,
                    colour: Rgb::BLACK,
                    outline: outline.filter(|o| !o.is_empty()),
                    attributes,
                };
                self.push(make(ItemKind::Text(text)));
            }
            "Bitmap" => {
                let w = e.attr_f64("W").unwrap_or(0.0);
                let h = e.attr_f64("H").unwrap_or(0.0);
                let Some(data) = e.attr("Data") else {
                    self.out.warn("A bitmap without embedded data was skipped");
                    return;
                };
                let decoded = B64.decode(data.trim().as_bytes()).ok();
                let img = decoded.and_then(|d| guard("image", || image::load_from_memory(&d)).ok().and_then(Result::ok));
                let Some(img) = img else {
                    self.out.warn("An embedded bitmap could not be decoded and was skipped");
                    return;
                };
                let mut attributes = BTreeMap::new();
                for (k, v) in &e.attrs {
                    if !matches!(k.as_str(), "Type" | "Data" | "W" | "H" | "CutIndex") {
                        attributes.insert(k.clone(), v.clone());
                    }
                }
                let transform = xf * Affine::translate((-w / 2.0, -h / 2.0)) * Affine::scale_non_uniform(w, h);
                self.push(make(ItemKind::Image(ImageItem { image: img, transform, attributes })));
            }
            "Group" => {
                let g = self.out.new_group(group, None);
                if let Some(children) = e.child("Children") {
                    for c in children.children.iter().filter(|c| c.name == "Shape") {
                        self.shape(c, xf, Some(g), depth + 1);
                    }
                }
            }
            other => {
                // Unknown shapes that still carry path data are read as paths.
                if e.child("VertList").is_some() || e.child("V").is_some() {
                    let p = self.path_geometry(e, xf);
                    if !p.is_empty() {
                        self.push(make(path_kind(p)));
                        return;
                    }
                }
                self.out.warn(format!("LightBurn shape type \"{other}\" is not supported and was skipped"));
            }
        }
    }

    /// Reads vertices and primitives (v0 `<V>`/`<P>` or v1 `VertList`/`PrimList`, with shared ids).
    fn path_geometry(&mut self, e: &Element, xf: Affine) -> BezPath {
        let mut verts: Option<Vec<Vert>> = None;
        let mut prims: Option<PrimSpec> = None;
        if let Some(vl) = e.child("VertList") {
            if vl.children.iter().any(|c| c.name == "V") {
                verts = Some(vl.children_named("V").map(v0_vert).collect());
            } else {
                verts = Some(parse_vert_list(&vl.text));
            }
        } else if e.child("V").is_some() {
            verts = Some(e.children_named("V").map(v0_vert).collect());
        }
        if let Some(pl) = e.child("PrimList") {
            if pl.children.iter().any(|c| c.name == "P") {
                prims = Some(PrimSpec::List(pl.children_named("P").filter_map(v0_prim).collect()));
            } else {
                prims = Some(parse_prim_list(&pl.text));
            }
        } else if e.child("P").is_some() {
            prims = Some(PrimSpec::List(e.children_named("P").filter_map(v0_prim).collect()));
        }
        if let Some(id) = e.attr("VertID") {
            match &verts {
                Some(v) => {
                    self.verts.insert(id.to_string(), v.clone());
                }
                None => verts = self.verts.get(id).cloned(),
            }
        }
        if let Some(id) = e.attr("PrimID") {
            match &prims {
                Some(p) => {
                    self.prims.insert(id.to_string(), p.clone());
                }
                None => prims = self.prims.get(id).cloned(),
            }
        }
        let verts = verts.unwrap_or_default();
        let prims = prims.unwrap_or(PrimSpec::AutoClosed);
        let mut p = build_path(&verts, &prims);
        p.apply_affine(xf);
        if p.is_finite() { p } else { BezPath::new() }
    }
}

fn path_kind(path: BezPath) -> ItemKind {
    ItemKind::Path(PathItem { path, stroke: None, fill: None, dash: None })
}

fn v0_vert(v: &Element) -> Vert {
    let p = Point::new(v.attr_f64("vx").unwrap_or(0.0), v.attr_f64("vy").unwrap_or(0.0));
    let c0 = v.attr_f64("c0x").zip(v.attr_f64("c0y")).map(Point::from);
    let c1 = v.attr_f64("c1x").zip(v.attr_f64("c1y")).map(Point::from);
    Vert { p, c0, c1 }
}

fn v0_prim(p: &Element) -> Option<Prim> {
    let a = p.attr_f64("p0")?.max(0.0) as usize;
    let b = p.attr_f64("p1")?.max(0.0) as usize;
    match p.attr("T").unwrap_or("L").trim() {
        "B" | "1" => Some(Prim::Bez(a, b)),
        _ => Some(Prim::Line(a, b)),
    }
}

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    V,
    S,
    Ctl(&'a str),
    Num(f64),
}

fn tokenize_verts(s: &str) -> Vec<Tok<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'V' {
            out.push(Tok::V);
            i += 1;
        } else if c == b'S' {
            out.push(Tok::S);
            i += 1;
        } else if c == b'c' && i + 3 <= b.len() && matches!(&s[i..i + 3], "c0x" | "c0y" | "c1x" | "c1y") {
            out.push(Tok::Ctl(&s[i..i + 3]));
            i += 3;
        } else if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() || d == b'.' {
                    i += 1;
                } else if (d == b'e' || d == b'E') && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'-' || b[i + 1] == b'+') {
                    i += 2;
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse::<f64>()
                && v.is_finite()
            {
                out.push(Tok::Num(v));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Parses a v1 `VertList`: `V<x> <y>` vertices with optional `c0x c0y c1x c1y` handles.
/// A handle with only its x part (`c0x1`) means "no control point".
fn parse_vert_list(s: &str) -> Vec<Vert> {
    let mut out: Vec<Vert> = Vec::new();
    let mut cur: Option<(Vert, [Option<f64>; 4])> = None;
    let mut target: Option<usize> = None; // 0..4 = c0x c0y c1x c1y, 4 = vx, 5 = vy
    let commit = |cur: Option<(Vert, [Option<f64>; 4])>, out: &mut Vec<Vert>| {
        if let Some((mut v, h)) = cur {
            if let (Some(x), Some(y)) = (h[0], h[1]) {
                v.c0 = Some(Point::new(x, y));
            }
            if let (Some(x), Some(y)) = (h[2], h[3]) {
                v.c1 = Some(Point::new(x, y));
            }
            out.push(v);
        }
    };
    for t in tokenize_verts(s) {
        match t {
            Tok::V => {
                commit(cur.take(), &mut out);
                cur = Some((Vert::default(), [None; 4]));
                target = Some(4);
            }
            Tok::S => {}
            Tok::Ctl(c) => {
                target = Some(match c {
                    "c0x" => 0,
                    "c0y" => 1,
                    "c1x" => 2,
                    _ => 3,
                })
            }
            Tok::Num(v) => {
                if let Some((vert, h)) = cur.as_mut() {
                    match target {
                        Some(4) => {
                            vert.p.x = v;
                            target = Some(5);
                        }
                        Some(5) => {
                            vert.p.y = v;
                            target = None;
                        }
                        Some(k @ 0..=3) => {
                            h[k] = Some(v);
                            target = None;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    commit(cur, &mut out);
    out
}

/// Parses a v1 `PrimList` (`L0 1L1 2B2 3`, `LineClosed`, or empty).
fn parse_prim_list(s: &str) -> PrimSpec {
    let t = s.trim();
    if t.is_empty() {
        return PrimSpec::AutoClosed;
    }
    if t == "LineClosed" {
        return PrimSpec::LineClosed;
    }
    let mut prims = Vec::new();
    let b = t.as_bytes();
    let mut i = 0;
    let mut kind = b'L';
    let read_int = |i: &mut usize| -> Option<usize> {
        while *i < b.len() && !b[*i].is_ascii_digit() && b[*i] != b'L' && b[*i] != b'B' {
            *i += 1;
        }
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        t[start..*i].parse().ok()
    };
    while i < b.len() {
        match b[i] {
            b'L' | b'B' => {
                kind = b[i];
                i += 1;
            }
            c if c.is_ascii_digit() => {
                let (Some(a), Some(c)) = (read_int(&mut i), read_int(&mut i)) else { break };
                prims.push(if kind == b'B' { Prim::Bez(a, c) } else { Prim::Line(a, c) });
            }
            _ => i += 1,
        }
    }
    PrimSpec::List(prims)
}

fn build_path(verts: &[Vert], prims: &PrimSpec) -> BezPath {
    let n = verts.len();
    let mut p = BezPath::new();
    if n == 0 {
        return p;
    }
    let list: Vec<Prim> = match prims {
        PrimSpec::List(l) => l.clone(),
        PrimSpec::LineClosed => (0..n).map(|i| Prim::Line(i, (i + 1) % n)).collect(),
        PrimSpec::AutoClosed => (0..n)
            .map(|i| {
                let j = (i + 1) % n;
                if verts[i].c0.is_some() && verts[j].c1.is_some() { Prim::Bez(i, j) } else { Prim::Line(i, j) }
            })
            .collect(),
    };
    let mut sub_start: Option<usize> = None;
    let mut last_end: Option<usize> = None;
    for prim in list {
        let (a, b) = match prim {
            Prim::Line(a, b) | Prim::Bez(a, b) => (a, b),
        };
        if a >= n || b >= n {
            continue;
        }
        if last_end != Some(a) {
            if let (Some(s), Some(e)) = (sub_start, last_end)
                && s == e
            {
                p.close_path();
            }
            p.move_to(verts[a].p);
            sub_start = Some(a);
        }
        match prim {
            Prim::Line(..) => p.line_to(verts[b].p),
            Prim::Bez(..) => {
                let c0 = verts[a].c0.unwrap_or(verts[a].p);
                let c1 = verts[b].c1.unwrap_or(verts[b].p);
                p.curve_to(c0, c1, verts[b].p);
            }
        }
        last_end = Some(b);
    }
    if let (Some(s), Some(e)) = (sub_start, last_end)
        && s == e
    {
        p.close_path();
    }
    p
}

// ---------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------

/// Options for writing `.lbrn2`.
#[derive(Clone, Debug)]
pub struct LbrnWriteOptions {
    /// Value of the `AppVersion` attribute (a format compatibility marker).
    pub app_version: String,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub material_height: f64,
}

impl Default for LbrnWriteOptions {
    fn default() -> Self {
        LbrnWriteOptions { app_version: "1.4.04".into(), mirror_x: false, mirror_y: true, material_height: 0.0 }
    }
}

/// Writes items, groups, layer settings and notes as a `.lbrn2` project (FormatVersion 1).
///
/// Items without a `layer_index` are put on a layer per distinct colour, after the indices the
/// layer settings already use. Paths are written as `Path` shapes in drawing coordinates.
pub fn write_lbrn2(doc: &Imported, opts: &LbrnWriteOptions) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<LightBurnProject AppVersion=\"{}\" FormatVersion=\"1\" MaterialHeight=\"{}\" MirrorX=\"{}\" MirrorY=\"{}\">\n",
        xml::escape(&opts.app_version),
        fmt_num(opts.material_height, 4),
        if opts.mirror_x { "True" } else { "False" },
        if opts.mirror_y { "True" } else { "False" }
    ));
    if let Some(png) = doc.project.as_ref().and_then(|p| p.thumbnail_png.as_ref()) {
        s.push_str(&format!("    <Thumbnail Source=\"{}\"/>\n", B64.encode(png)));
    }
    if let Some(p) = &doc.project {
        for (tag, map) in [("VariableText", &p.variable_text), ("UIPrefs", &p.ui_prefs)] {
            if !map.is_empty() {
                s.push_str(&format!("    <{tag}>\n"));
                for (k, v) in map {
                    if is_xml_name(k) {
                        s.push_str(&format!("        <{k} Value=\"{}\"/>\n", xml::escape(v)));
                    }
                }
                s.push_str(&format!("    </{tag}>\n"));
            }
        }
    }

    // Layer assignment.
    let mut settings = doc.layer_settings.clone();
    let mut colour_layers: Vec<(Rgb, u32)> = Vec::new();
    let mut next = settings.iter().map(|l| l.index + 1).max().unwrap_or(0);
    let mut item_layer = Vec::with_capacity(doc.items.len());
    for item in &doc.items {
        let idx = match item.layer_index {
            Some(i) => i,
            None => {
                let c = item.colour().unwrap_or(Rgb::BLACK);
                match colour_layers.iter().find(|(k, _)| *k == c) {
                    Some((_, i)) => *i,
                    None => {
                        let i = next;
                        next += 1;
                        colour_layers.push((c, i));
                        i
                    }
                }
            }
        };
        if !settings.iter().any(|l| l.index == idx) {
            let mode = if matches!(item.kind, ItemKind::Image(_)) { "Image" } else { "Cut" };
            let name = item.layer.clone().unwrap_or_else(|| format!("C{idx:02}"));
            settings.push(LayerSettings { index: idx, name, mode: mode.into(), values: BTreeMap::new() });
        }
        item_layer.push(idx);
    }
    settings.sort_by_key(|l| (l.index, l.values.contains_key("tag")));
    for l in &settings {
        let tag = if l.values.get("tag").map(String::as_str) == Some("CutSetting_Img") { "CutSetting_Img" } else { "CutSetting" };
        s.push_str(&format!("    <{tag} type=\"{}\">\n", xml::escape(&l.mode)));
        s.push_str(&format!("        <index Value=\"{}\"/>\n", l.index));
        s.push_str(&format!("        <name Value=\"{}\"/>\n", xml::escape(&l.name)));
        for (k, v) in &l.values {
            if matches!(k.as_str(), "index" | "name" | "tag") || !is_xml_name(k) {
                continue;
            }
            s.push_str(&format!("        <{k} Value=\"{}\"/>\n", xml::escape(v)));
        }
        s.push_str(&format!("    </{tag}>\n"));
    }

    // Shapes, nested by group.
    let mut w = ShapeWriter { out: String::new(), next_id: 0 };
    let mut children: HashMap<Option<u32>, Vec<Node>> = HashMap::new();
    let parent_of: HashMap<u32, Option<u32>> = doc.groups.iter().map(|g| (g.id, g.parent)).collect();
    let mut seen_groups = std::collections::HashSet::new();
    for (i, item) in doc.items.iter().enumerate() {
        // Register the chain of groups of this item.
        let mut g = item.group;
        let mut chain_guard = 0;
        while let Some(id) = g {
            chain_guard += 1;
            if chain_guard > 64 || !seen_groups.insert(id) {
                break;
            }
            let parent = parent_of.get(&id).copied().flatten();
            children.entry(parent).or_default().push(Node::Group(id));
            g = parent;
        }
        children.entry(item.group.filter(|g| parent_of.contains_key(g))).or_default().push(Node::Item(i));
    }
    w.nodes(&children, None, doc, &item_layer, 1, &mut Vec::new());
    s.push_str(&w.out);

    if let Some(p) = &doc.project
        && let Some(n) = &p.notes
    {
        s.push_str(&format!("    <Notes ShowOnLoad=\"{}\" Notes=\"{}\"/>\n", if p.show_notes_on_load { 1 } else { 0 }, xml::escape(n)));
    }
    s.push_str("</LightBurnProject>\n");
    s
}

fn is_xml_name(k: &str) -> bool {
    let mut c = k.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
}

#[derive(Clone, Copy)]
enum Node {
    Group(u32),
    Item(usize),
}

struct ShapeWriter {
    out: String,
    next_id: u32,
}

fn xform_text(a: Affine) -> String {
    a.as_coeffs().iter().map(|v| fmt_num(*v, 6)).collect::<Vec<_>>().join(" ")
}

impl ShapeWriter {
    fn nodes(&mut self, tree: &HashMap<Option<u32>, Vec<Node>>, at: Option<u32>, doc: &Imported, layers: &[u32], depth: usize, stack: &mut Vec<u32>) {
        let Some(list) = tree.get(&at) else { return };
        let pad = "    ".repeat(depth);
        for node in list {
            match *node {
                Node::Item(i) => self.item(&doc.items[i], layers[i], &pad),
                Node::Group(g) => {
                    if stack.contains(&g) || depth > 64 {
                        continue;
                    }
                    stack.push(g);
                    self.out.push_str(&format!("{pad}<Shape Type=\"Group\">\n{pad}    <XForm>1 0 0 1 0 0</XForm>\n{pad}    <Children>\n"));
                    self.nodes(tree, Some(g), doc, layers, depth + 2, stack);
                    self.out.push_str(&format!("{pad}    </Children>\n{pad}</Shape>\n"));
                    stack.pop();
                }
            }
        }
    }

    fn path_body(&mut self, path: &BezPath, pad: &str) -> Option<String> {
        let (verts, prims) = encode_path(path);
        if prims.is_empty() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        Some(format!(
            " VertID=\"{id}\" PrimID=\"{id}\">\n{pad}    <XForm>1 0 0 1 0 0</XForm>\n{pad}    <VertList>{verts}</VertList>\n{pad}    <PrimList>{prims}</PrimList>\n"
        ))
    }

    fn item(&mut self, item: &Item, layer: u32, pad: &str) {
        let hidden = if item.hidden { " Hidden=\"1\"" } else { "" };
        match &item.kind {
            ItemKind::Path(p) => {
                if let Some(body) = self.path_body(&p.path, pad) {
                    self.out.push_str(&format!("{pad}<Shape Type=\"Path\" CutIndex=\"{layer}\"{hidden}{body}{pad}</Shape>\n"));
                }
            }
            ItemKind::Text(t) => {
                let font = t.style.clone().filter(|s| s.contains(',')).unwrap_or_else(|| t.font.clone());
                let ah = match t.h_align {
                    HAlign::Left => 0,
                    HAlign::Center => 1,
                    HAlign::Right => 2,
                };
                let backup = t.outline.as_ref().and_then(|o| {
                    let inv = t.transform.inverse();
                    let mut local = o.clone();
                    local.apply_affine(inv);
                    local.is_finite().then_some(local)
                });
                let has_backup = backup.is_some();
                self.out.push_str(&format!(
                    "{pad}<Shape Type=\"Text\" CutIndex=\"{layer}\"{hidden} Str=\"{}\" Font=\"{}\" H=\"{}\" Ah=\"{ah}\" HasBackupPath=\"{}\">\n{pad}    <XForm>{}</XForm>\n",
                    xml::escape(&t.text),
                    xml::escape(&font),
                    fmt_num(t.height, 6),
                    if has_backup { 1 } else { 0 },
                    xform_text(t.transform)
                ));
                if let Some(b) = backup {
                    let inner = format!("{pad}    ");
                    if let Some(body) = self.path_body(&b, &inner) {
                        self.out.push_str(&format!("{inner}<BackupPath Type=\"Path\" CutIndex=\"{layer}\"{body}{inner}</BackupPath>\n"));
                    }
                }
                self.out.push_str(&format!("{pad}</Shape>\n"));
            }
            ItemKind::Image(img) => {
                let t = img.transform.as_coeffs();
                let w = (t[0] * t[0] + t[1] * t[1]).sqrt();
                let h = (t[2] * t[2] + t[3] * t[3]).sqrt();
                if !(w > 0.0 && h > 0.0) {
                    return;
                }
                let local = img.transform * Affine::translate((0.5, 0.5)) * Affine::scale_non_uniform(1.0 / w, 1.0 / h);
                let mut png = Vec::new();
                if img.image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
                    return;
                }
                let mut extra = String::new();
                for (k, v) in &img.attributes {
                    if is_xml_name(k) && !matches!(k.as_str(), "Hidden") {
                        extra.push_str(&format!(" {k}=\"{}\"", xml::escape(v)));
                    }
                }
                self.out.push_str(&format!(
                    "{pad}<Shape Type=\"Bitmap\" CutIndex=\"{layer}\"{hidden} W=\"{}\" H=\"{}\"{extra} Data=\"{}\">\n{pad}    <XForm>{}</XForm>\n{pad}</Shape>\n",
                    fmt_num(w, 6),
                    fmt_num(h, 6),
                    B64.encode(&png),
                    xform_text(local)
                ));
            }
        }
    }
}

/// Encodes a path as v1 `VertList` / `PrimList` strings.
fn encode_path(path: &BezPath) -> (String, String) {
    let mut verts: Vec<Vert> = Vec::new();
    let mut prims: Vec<Prim> = Vec::new();
    for sub in geom::split_subpaths(path) {
        let closed = geom::is_closed(&sub);
        let start = verts.len();
        let mut cur = start;
        let els: Vec<PathEl> = sub.elements().to_vec();
        let first = match els.first() {
            Some(PathEl::MoveTo(p)) => *p,
            _ => continue,
        };
        verts.push(Vert { p: first, c0: None, c1: None });
        let mut last_pt = first;
        let seg_count = els.iter().filter(|e| !matches!(e, PathEl::MoveTo(_) | PathEl::ClosePath)).count();
        let mut seen = 0;
        for el in &els[1..] {
            let (c0, c1, end, bez) = match *el {
                PathEl::LineTo(p) => (None, None, p, false),
                PathEl::QuadTo(q, p) => {
                    let c0 = last_pt + (q - last_pt) * (2.0 / 3.0);
                    let c1 = p + (q - p) * (2.0 / 3.0);
                    (Some(c0), Some(c1), p, true)
                }
                PathEl::CurveTo(a, b, p) => (Some(a), Some(b), p, true),
                _ => continue,
            };
            seen += 1;
            // The last segment of a closed subpath ending on the start re-uses vertex `start`.
            let to = if closed && seen == seg_count && (end - first).hypot() < 1e-9 {
                start
            } else {
                verts.push(Vert { p: end, c0: None, c1: None });
                verts.len() - 1
            };
            if bez {
                verts[cur].c0 = c0;
                verts[to].c1 = c1;
                prims.push(Prim::Bez(cur, to));
            } else {
                prims.push(Prim::Line(cur, to));
            }
            cur = to;
            last_pt = end;
        }
        if closed && cur != start {
            prims.push(Prim::Line(cur, start));
        }
    }
    let mut vs = String::new();
    for v in &verts {
        vs.push_str(&format!("V{} {}", fmt_num(v.p.x, 6), fmt_num(v.p.y, 6)));
        match v.c0 {
            Some(c) => vs.push_str(&format!("c0x{}c0y{}", fmt_num(c.x, 6), fmt_num(c.y, 6))),
            None => vs.push_str("c0x1"),
        }
        match v.c1 {
            Some(c) => vs.push_str(&format!("c1x{}c1y{}", fmt_num(c.x, 6), fmt_num(c.y, 6))),
            None => vs.push_str("c1x1"),
        }
    }
    let mut ps = String::new();
    for p in &prims {
        match p {
            Prim::Line(a, b) => ps.push_str(&format!("L{a} {b}")),
            Prim::Bez(a, b) => ps.push_str(&format!("B{a} {b}")),
        }
    }
    (vs, ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vert_list_handles_and_placeholders() {
        let v = parse_vert_list("V0 0c0x1c1x1V10 0c0x12c0y3c1x8c1y-2SV1e1 5c0x1");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].p, Point::new(0.0, 0.0));
        assert!(v[0].c0.is_none() && v[0].c1.is_none());
        assert_eq!(v[1].c0, Some(Point::new(12.0, 3.0)));
        assert_eq!(v[1].c1, Some(Point::new(8.0, -2.0)));
        assert_eq!(v[2].p, Point::new(10.0, 5.0));
    }

    #[test]
    fn prim_list_forms() {
        match parse_prim_list("L0 1L1 2B2 0") {
            PrimSpec::List(l) => assert_eq!(l.len(), 3),
            _ => panic!(),
        }
        assert!(matches!(parse_prim_list("LineClosed"), PrimSpec::LineClosed));
        assert!(matches!(parse_prim_list(" "), PrimSpec::AutoClosed));
    }
}
