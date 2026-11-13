//! DXF import (via the IxMilia `dxf` crate, MIT).
//!
//! Units come from `$INSUNITS`, then `$MEASUREMENT`, then [`DxfOptions::fallback_mm_per_unit`].
//! Lines, arcs, circles, ellipses, (LW)polylines with bulges, splines/NURBS (as Béziers),
//! blocks and nested inserts (with arrays), dimensions (their anonymous blocks), TEXT, MTEXT
//! and ATTRIB (as text records) are read. Frozen or switched-off layers are skipped.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;

use dxf::entities::{Entity, EntityType};
use dxf::enums::{AttachmentPoint, HorizontalTextJustification, Units, VerticalTextJustification};
use dxf::{Block, Drawing};
use kurbo::{Affine, BezPath, Point, Vec2};

use crate::geom::{self, arc_path, bulge_to, circle_path};
use crate::model::*;
use crate::{ImportError, guard};

/// DXF import options.
#[derive(Clone, Debug)]
pub struct DxfOptions {
    /// Millimetres per drawing unit when the file states no units. Default 1 (mm).
    pub fallback_mm_per_unit: f64,
    /// Forces a scale (mm per unit), ignoring what the file says.
    pub force_mm_per_unit: Option<f64>,
    /// Joins open paths whose ends are closer than this (mm). `None` = off.
    pub auto_join_tolerance: Option<f64>,
    /// Put each block insert in its own group.
    pub group_blocks: bool,
}

impl Default for DxfOptions {
    fn default() -> Self {
        DxfOptions { fallback_mm_per_unit: 1.0, force_mm_per_unit: None, auto_join_tolerance: None, group_blocks: true }
    }
}

/// Millimetres per unit for a DXF `$INSUNITS` code. `None` for unitless or unknown codes.
pub fn units_to_mm(code: i32) -> Option<f64> {
    Some(match code {
        1 => 25.4,
        2 => 304.8,
        3 => 1_609_344.0,
        4 => 1.0,
        5 => 10.0,
        6 => 1000.0,
        7 => 1_000_000.0,
        8 => 25.4e-6,
        9 => 0.0254,
        10 => 914.4,
        11 => 1e-7,
        12 => 1e-6,
        13 => 1e-3,
        14 => 100.0,
        15 => 10_000.0,
        16 => 100_000.0,
        17 => 1e12,
        18 => 1.495_978_707e14,
        19 => 9.460_730_472_580_8e18,
        20 => 3.085_677_581_49e19,
        21 => 304.800_609_6,
        22 => 25.400_050_8,
        23 => 914.401_828_8,
        24 => 1_609_347.218_694_4,
        _ => return None,
    })
}

/// The AutoCAD Colour Index palette as RGB. Index 7 (white/black) maps to black, since
/// laser work is shown on a light background. 0 and 256 (by block / by layer) give black.
pub fn aci_to_rgb(i: u8) -> Rgb {
    match i {
        1 => Rgb(255, 0, 0),
        2 => Rgb(255, 255, 0),
        3 => Rgb(0, 255, 0),
        4 => Rgb(0, 255, 255),
        5 => Rgb(0, 0, 255),
        6 => Rgb(255, 0, 255),
        8 => Rgb(128, 128, 128),
        9 => Rgb(192, 192, 192),
        10..=249 => {
            let hue = f64::from((i - 10) / 10) * 15.0;
            let sub = (i - 10) % 10;
            let value = [1.0, 1.0, 0.8, 0.8, 0.6, 0.6, 0.5, 0.5, 0.3, 0.3][sub as usize];
            let sat = if sub.is_multiple_of(2) { 1.0 } else { 0.5 };
            hsv(hue, sat, value)
        }
        250 => Rgb(51, 51, 51),
        251 => Rgb(80, 80, 80),
        252 => Rgb(105, 105, 105),
        253 => Rgb(130, 130, 130),
        254 => Rgb(190, 190, 190),
        255 => Rgb(255, 255, 255),
        _ => Rgb::BLACK,
    }
}

/// The ACI index whose colour is nearest to `c` (1..=255; black maps to 7).
pub fn nearest_aci(c: Rgb) -> u8 {
    if c == Rgb::BLACK {
        return 7;
    }
    let mut best = (u32::MAX, 7u8);
    for i in 1..=255u8 {
        if i == 7 {
            continue;
        }
        let d = aci_to_rgb(i).distance2(c);
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

fn hsv(h: f64, s: f64, v: f64) -> Rgb {
    let c = v * s;
    let hp = (h / 60.0) % 6.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Rgb::from_unit(r + m, g + m, b + m)
}

#[derive(Clone, Debug, Default)]
struct RawLayer {
    flags: i32,
    color: Option<i32>,
    true_color: Option<u32>,
}

#[derive(Debug, Default)]
struct RawScan {
    insunits: Option<i32>,
    measurement: Option<i32>,
    layers: HashMap<String, RawLayer>,
}

/// Reads what the `dxf` crate does not expose (layer flags and true colours, `$MEASUREMENT`)
/// straight from the ASCII group codes.
fn raw_scan(bytes: &[u8]) -> RawScan {
    let mut scan = RawScan::default();
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return scan;
    }
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let mut pairs = Vec::new();
    while let (Some(c), Some(v)) = (lines.next(), lines.next()) {
        let Ok(code) = c.trim().parse::<i32>() else { break };
        pairs.push((code, v.trim()));
    }
    let mut i = 0;
    let mut var = "";
    let mut layer: Option<(String, RawLayer)> = None;
    let mut in_layer = false;
    while i < pairs.len() {
        let (code, value) = pairs[i];
        if code == 2 && value == "ENTITIES" && !in_layer {
            // Header and tables come first; the rest is not needed here.
            break;
        }
        if code == 9 {
            var = value;
        } else if code == 70 && var == "$INSUNITS" {
            scan.insunits = value.parse().ok();
            var = "";
        } else if code == 70 && var == "$MEASUREMENT" {
            scan.measurement = value.parse().ok();
            var = "";
        }
        if code == 0 {
            if let Some((name, l)) = layer.take() {
                scan.layers.insert(name, l);
            }
            in_layer = value == "LAYER";
            if in_layer {
                layer = Some((String::new(), RawLayer::default()));
            }
        } else if in_layer && let Some((name, l)) = layer.as_mut() {
            match code {
                2 => *name = value.to_string(),
                70 => l.flags = value.parse().unwrap_or(0),
                62 => l.color = value.parse().ok(),
                420 => l.true_color = value.parse::<i64>().ok().map(|v| (v & 0xFF_FFFF) as u32),
                _ => {}
            }
        }
        i += 1;
    }
    if let Some((name, l)) = layer.take() {
        scan.layers.insert(name, l);
    }
    scan
}

struct LayerInfo {
    colour: Rgb,
    hidden: bool,
    line_type: String,
}

struct Ctx<'a> {
    opts: &'a DxfOptions,
    blocks: HashMap<String, &'a Block>,
    layers: HashMap<String, LayerInfo>,
    styles: HashMap<String, String>,
    line_types: HashMap<String, Vec<f64>>,
    scale: f64,
    out: Imported,
    skipped: BTreeMap<&'static str, usize>,
    hidden_skipped: usize,
    budget: usize,
    /// Block copies that may still be expanded (see [`MAX_INSERTS`]).
    insert_budget: usize,
}

/// Inherited state when walking into a block.
#[derive(Clone)]
struct Inherit {
    xf: Affine,
    layer: Option<String>,
    colour: Option<Rgb>,
    line_type: Option<String>,
    group: Option<u32>,
    depth: u32,
}

const MAX_DEPTH: u32 = 32;
const MAX_ITEMS: usize = 2_000_000;
/// The most block copies expanded in one file. Blocks that insert each other several times
/// multiply; without a limit a tiny file can ask for billions of copies.
const MAX_INSERTS: usize = 200_000;

/// Imports a DXF file (ASCII or binary).
pub fn import(bytes: &[u8], opts: &DxfOptions) -> Result<Imported, ImportError> {
    let drawing = guard("DXF", || Drawing::load(&mut Cursor::new(bytes)))?.map_err(|e| ImportError::Parse(format!("DXF: {e}")))?;
    let raw = raw_scan(bytes);

    let mut out = Imported::default();
    let insunits = raw.insunits.unwrap_or(drawing.header.default_drawing_units as i32);
    let scale = if let Some(f) = opts.force_mm_per_unit {
        f
    } else if let Some(s) = units_to_mm(insunits) {
        s
    } else if let Some(m) = raw.measurement {
        if m == 0 { 25.4 } else { 1.0 }
    } else {
        if drawing.header.default_drawing_units == Units::Unitless {
            out.warn(format!("The DXF does not say which units it uses; assumed {} mm per unit", geom::fmt_num(opts.fallback_mm_per_unit, 4)));
        }
        opts.fallback_mm_per_unit
    };
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };

    let mut layers = HashMap::new();
    for l in drawing.layers() {
        let raw_l = raw.layers.get(&l.name).cloned().unwrap_or_default();
        let index = raw_l.color.unwrap_or(1);
        let colour = match raw_l.true_color {
            Some(tc) => Rgb::from_packed(tc),
            None => l.color.index().map(aci_to_rgb).unwrap_or_else(|| aci_to_rgb(index.unsigned_abs().min(255) as u8)),
        };
        let off = l.color.is_turned_off() || raw_l.color.is_some_and(|c| c < 0) || !l.is_layer_on;
        let frozen = raw_l.flags & 1 != 0;
        layers.insert(l.name.clone(), LayerInfo { colour, hidden: off || frozen, line_type: l.line_type_name.clone() });
    }
    let styles = drawing
        .styles()
        .map(|s| {
            let font = if s.primary_font_file_name.is_empty() { s.name.clone() } else { s.primary_font_file_name.clone() };
            (s.name.to_ascii_uppercase(), font)
        })
        .collect();
    let line_types = drawing
        .line_types()
        .filter(|lt| lt.dash_dot_space_lengths.iter().any(|v| *v != 0.0))
        .map(|lt| (lt.name.to_ascii_uppercase(), lt.dash_dot_space_lengths.clone()))
        .collect();
    let blocks = drawing.blocks().map(|b| (b.name.to_ascii_uppercase(), b)).collect();

    let mut ctx = Ctx {
        opts,
        blocks,
        layers,
        styles,
        line_types,
        scale,
        out,
        skipped: BTreeMap::new(),
        hidden_skipped: 0,
        budget: MAX_ITEMS,
        insert_budget: MAX_INSERTS,
    };
    let root = Inherit { xf: Affine::scale(scale), layer: None, colour: None, line_type: None, group: None, depth: 0 };
    for e in drawing.entities() {
        if e.common.is_in_paper_space {
            continue;
        }
        ctx.entity(e, &root);
    }
    let mut out = ctx.out;
    if ctx.hidden_skipped > 0 {
        out.warn(format!("{} objects on hidden or frozen layers were left out", ctx.hidden_skipped));
    }
    for (kind, n) in ctx.skipped {
        out.warn(format!("{n} {kind} entities are not supported and were skipped"));
    }
    if let Some(tol) = opts.auto_join_tolerance {
        geom::join_open_paths(&mut out.items, tol);
    }
    Ok(crate::finish(out))
}

fn ocs(normal_z: f64) -> Affine {
    if normal_z < 0.0 { Affine::scale_non_uniform(-1.0, 1.0) } else { Affine::IDENTITY }
}

fn pt(p: &dxf::Point) -> Point {
    Point::new(p.x, p.y)
}

impl Ctx<'_> {
    fn effective_layer(&self, e: &Entity, inh: &Inherit) -> String {
        if e.common.layer == "0"
            && let Some(l) = &inh.layer
        {
            return l.clone();
        }
        e.common.layer.clone()
    }

    fn colour(&self, e: &Entity, layer: &str, inh: &Inherit) -> Rgb {
        let c = &e.common.color;
        if e.common.color_24_bit != 0 && !c.is_by_layer() && !c.is_by_block() {
            return Rgb::from_packed(e.common.color_24_bit as u32);
        }
        let layer_colour = self.layers.get(layer).map(|l| l.colour).unwrap_or(Rgb::BLACK);
        if c.is_by_layer() {
            layer_colour
        } else if c.is_by_block() {
            inh.colour.unwrap_or(layer_colour)
        } else if let Some(i) = c.index() {
            aci_to_rgb(i)
        } else {
            layer_colour
        }
    }

    fn dash(&self, e: &Entity, layer: &str, inh: &Inherit) -> Option<Vec<f64>> {
        let mut name = e.common.line_type_name.to_ascii_uppercase();
        if name.is_empty() || name == "BYLAYER" {
            name = self.layers.get(layer).map(|l| l.line_type.to_ascii_uppercase()).unwrap_or_default();
        } else if name == "BYBLOCK" {
            name = inh.line_type.clone().unwrap_or_default().to_ascii_uppercase();
        }
        let pattern = self.line_types.get(&name)?;
        let s = self.scale * if e.common.line_type_scale > 0.0 { e.common.line_type_scale } else { 1.0 };
        Some(pattern.iter().map(|v| v.abs() * s).collect())
    }

    fn push_path(&mut self, e: &Entity, inh: &Inherit, mut path: BezPath, local: Affine) {
        if path.is_empty() || self.budget == 0 {
            return;
        }
        self.budget -= 1;
        path.apply_affine(inh.xf * local);
        if !path.is_finite() {
            return;
        }
        let layer = self.effective_layer(e, inh);
        let colour = self.colour(e, &layer, inh);
        let dash = self.dash(e, &layer, inh);
        if dash.is_some() {
            self.out.warn("Dashed line types are kept as a note; they cut as solid lines");
        }
        let mut item = Item::path(path, Some(colour), None).with_layer(layer).with_group(inh.group);
        if let ItemKind::Path(p) = &mut item.kind {
            p.dash = dash;
        }
        self.out.items.push(item);
    }

    fn skip(&mut self, kind: &'static str) {
        *self.skipped.entry(kind).or_default() += 1;
    }

    fn entity(&mut self, e: &Entity, inh: &Inherit) {
        if !e.common.is_visible {
            return;
        }
        let layer = self.effective_layer(e, inh);
        if self.layers.get(&layer).is_some_and(|l| l.hidden) {
            self.hidden_skipped += 1;
            return;
        }
        match &e.specific {
            EntityType::Line(l) => {
                let mut p = BezPath::new();
                p.move_to(pt(&l.p1));
                p.line_to(pt(&l.p2));
                // A line's points are world coordinates; its extrusion only steers a thickness.
                self.push_path(e, inh, p, Affine::IDENTITY);
            }
            EntityType::Circle(c) => {
                self.push_path(e, inh, circle_path(pt(&c.center), c.radius), ocs(c.normal.z));
            }
            EntityType::Arc(a) => {
                let start = a.start_angle.to_radians();
                let mut sweep = (a.end_angle - a.start_angle).rem_euclid(360.0).to_radians();
                if sweep <= 1e-12 {
                    sweep = std::f64::consts::TAU;
                }
                self.push_path(e, inh, arc_path(pt(&a.center), a.radius, a.radius, 0.0, start, sweep), ocs(a.normal.z));
            }
            EntityType::Ellipse(el) => {
                let major = Vec2::new(el.major_axis.x, el.major_axis.y);
                let rx = major.hypot();
                let ry = rx * el.minor_axis_ratio.abs();
                let rot = major.atan2();
                let tau = std::f64::consts::TAU;
                let mut sweep = (el.end_parameter - el.start_parameter).rem_euclid(tau);
                if sweep <= 1e-12 {
                    sweep = tau;
                }
                let full = (sweep - tau).abs() < 1e-9;
                let (start, sweep) = if el.normal.z < 0.0 { (-el.start_parameter, -sweep) } else { (el.start_parameter, sweep) };
                let mut p = arc_path(pt(&el.center), rx, ry, rot, start, sweep);
                if full && !p.is_empty() {
                    p.close_path();
                }
                self.push_path(e, inh, p, Affine::IDENTITY);
            }
            EntityType::LwPolyline(pl) => {
                let verts: Vec<(Point, f64)> = pl.vertices.iter().map(|v| (Point::new(v.x, v.y), v.bulge)).collect();
                let p = bulge_path(&verts, pl.is_closed());
                self.push_path(e, inh, p, ocs(pl.extrusion_direction.z));
            }
            EntityType::Polyline(pl) => {
                if pl.flags & (16 | 64) != 0 {
                    self.skip("polygon mesh / polyface");
                    return;
                }
                let verts: Vec<(Point, f64)> = pl.vertices().filter(|v| v.flags & 16 == 0).map(|v| (pt(&v.location), v.bulge)).collect();
                let p = bulge_path(&verts, pl.is_closed());
                let local = if pl.flags & 8 != 0 { Affine::IDENTITY } else { ocs(pl.normal.z) };
                self.push_path(e, inh, p, local);
            }
            EntityType::Spline(s) => {
                let (p, approx) = spline_path(s);
                if approx {
                    self.out.warn("Some splines were rational or above degree 3 and were approximated");
                }
                self.push_path(e, inh, p, Affine::IDENTITY);
            }
            EntityType::Solid(s) => {
                let c = [&s.first_corner, &s.second_corner, &s.third_corner, &s.fourth_corner];
                self.push_path(e, inh, quad_path(c), ocs(s.extrusion_direction.z));
            }
            EntityType::Trace(s) => {
                let c = [&s.first_corner, &s.second_corner, &s.third_corner, &s.fourth_corner];
                self.push_path(e, inh, quad_path(c), ocs(s.extrusion_direction.z));
            }
            EntityType::Text(t) => {
                let anchor = if at_first_point(t.horizontal_text_justification, t.vertical_text_justification) {
                    pt(&t.location)
                } else {
                    pt(&t.second_alignment_point)
                };
                let (h, v) = text_align(t.horizontal_text_justification, t.vertical_text_justification);
                let rec = TextRec {
                    text: decode_text_codes(&t.value),
                    style: t.text_style_name.clone(),
                    height: t.text_height,
                    anchor,
                    rotation: t.rotation,
                    width_factor: t.relative_x_scale_factor,
                    oblique: t.oblique_angle,
                    h,
                    v,
                    ocs_z: t.normal.z,
                };
                self.push_text(e, inh, rec);
            }
            EntityType::Attribute(a) => self.attribute(e, a, inh),
            EntityType::MText(m) => {
                let mut raw = m.extended_text.concat();
                raw.push_str(&m.text);
                let dir = Vec2::new(m.x_axis_direction.x, m.x_axis_direction.y);
                let rotation = if dir.hypot() > 1e-12 { dir.atan2().to_degrees() } else { m.rotation_angle.to_degrees() };
                let (h, v) = match m.attachment_point {
                    AttachmentPoint::TopLeft => (HAlign::Left, VAlign::Top),
                    AttachmentPoint::TopCenter => (HAlign::Center, VAlign::Top),
                    AttachmentPoint::TopRight => (HAlign::Right, VAlign::Top),
                    AttachmentPoint::MiddleLeft => (HAlign::Left, VAlign::Middle),
                    AttachmentPoint::MiddleCenter => (HAlign::Center, VAlign::Middle),
                    AttachmentPoint::MiddleRight => (HAlign::Right, VAlign::Middle),
                    AttachmentPoint::BottomLeft => (HAlign::Left, VAlign::Bottom),
                    AttachmentPoint::BottomCenter => (HAlign::Center, VAlign::Bottom),
                    AttachmentPoint::BottomRight => (HAlign::Right, VAlign::Bottom),
                };
                let rec = TextRec {
                    text: strip_mtext(&raw),
                    style: m.text_style_name.clone(),
                    height: m.initial_text_height,
                    anchor: pt(&m.insertion_point),
                    rotation,
                    width_factor: 1.0,
                    oblique: 0.0,
                    h,
                    v,
                    ocs_z: m.extrusion_direction.z,
                };
                self.push_text(e, inh, rec);
            }
            EntityType::Insert(ins) => self.insert(e, ins, inh),
            EntityType::RotatedDimension(d) => self.dimension(e, &d.dimension_base.block_name, inh),
            EntityType::RadialDimension(d) => self.dimension(e, &d.dimension_base.block_name, inh),
            EntityType::DiameterDimension(d) => self.dimension(e, &d.dimension_base.block_name, inh),
            EntityType::AngularThreePointDimension(d) => self.dimension(e, &d.dimension_base.block_name, inh),
            EntityType::OrdinateDimension(d) => self.dimension(e, &d.dimension_base.block_name, inh),
            EntityType::AttributeDefinition(_) | EntityType::Seqend(_) | EntityType::Vertex(_) => {}
            EntityType::ModelPoint(_) => self.skip("POINT"),
            EntityType::Image(_) => self.skip("IMAGE (external raster reference)"),
            EntityType::Face3D(_) | EntityType::Solid3D(_) | EntityType::Body(_) | EntityType::Region(_) => self.skip("3D"),
            EntityType::Leader(_) => self.skip("LEADER"),
            EntityType::MLine(_) => self.skip("MLINE"),
            EntityType::Helix(_) => self.skip("HELIX"),
            EntityType::Ray(_) | EntityType::XLine(_) => self.skip("construction line"),
            _ => self.skip("other"),
        }
    }

    fn attribute(&mut self, e: &Entity, a: &dxf::entities::Attribute, inh: &Inherit) {
        if a.flags & 1 != 0 {
            return; // invisible attribute
        }
        let anchor = if at_first_point(a.horizontal_text_justification, a.vertical_text_justification) {
            pt(&a.location)
        } else {
            pt(&a.second_alignment_point)
        };
        let (h, v) = text_align(a.horizontal_text_justification, a.vertical_text_justification);
        let rec = TextRec {
            text: decode_text_codes(&a.value),
            style: a.text_style_name.clone(),
            height: a.text_height,
            anchor,
            rotation: a.rotation,
            width_factor: a.relative_x_scale_factor,
            oblique: a.oblique_angle,
            h,
            v,
            ocs_z: a.normal.z,
        };
        self.push_text(e, inh, rec);
    }

    fn push_text(&mut self, e: &Entity, inh: &Inherit, rec: TextRec) {
        if rec.text.trim().is_empty() || self.budget == 0 {
            return;
        }
        self.budget -= 1;
        let layer = self.effective_layer(e, inh);
        let colour = self.colour(e, &layer, inh);
        let wf = if rec.width_factor > 0.0 { rec.width_factor } else { 1.0 };
        let transform = inh.xf
            * ocs(rec.ocs_z)
            * Affine::translate(rec.anchor.to_vec2())
            * Affine::rotate(rec.rotation.to_radians())
            * Affine::skew(rec.oblique.to_radians().tan(), 0.0)
            * Affine::scale_non_uniform(wf, 1.0);
        let style_key = rec.style.to_ascii_uppercase();
        let font = self.styles.get(&style_key).cloned().unwrap_or_else(|| rec.style.clone());
        let mut attributes = BTreeMap::new();
        if (wf - 1.0).abs() > 1e-9 {
            attributes.insert("widthFactor".into(), geom::fmt_num(wf, 4));
        }
        let text = TextItem {
            text: rec.text,
            font,
            style: (!rec.style.is_empty()).then_some(rec.style),
            height: rec.height,
            h_align: rec.h,
            v_align: rec.v,
            transform,
            colour,
            outline: None,
            attributes,
        };
        let item = Item::new(ItemKind::Text(text)).with_layer(layer).with_group(inh.group);
        self.out.items.push(item);
    }

    fn insert(&mut self, e: &Entity, ins: &dxf::entities::Insert, inh: &Inherit) {
        let Some(block) = self.blocks.get(&ins.name.to_ascii_uppercase()).copied() else {
            self.out.warn(format!("Block \"{}\" is missing", ins.name));
            return;
        };
        if inh.depth >= MAX_DEPTH {
            self.out.warn("Blocks nested too deeply (or referring to themselves) were cut off");
            return;
        }
        let layer = self.effective_layer(e, inh);
        let colour = self.colour(e, &layer, inh);
        let sx = if ins.x_scale_factor == 0.0 { 1.0 } else { ins.x_scale_factor };
        let sy = if ins.y_scale_factor == 0.0 { 1.0 } else { ins.y_scale_factor };
        let cols = ins.column_count.max(1) as i32;
        let rows = ins.row_count.max(1) as i32;
        if i64::from(cols) * i64::from(rows) > 10_000 {
            self.out.warn("A block array was too large and was cut down to its first copy");
        }
        let (cols, rows) = if i64::from(cols) * i64::from(rows) > 10_000 { (1, 1) } else { (cols, rows) };
        let line_type = Some(e.common.line_type_name.clone()).filter(|n| !n.eq_ignore_ascii_case("BYBLOCK"));
        for r in 0..rows {
            for c in 0..cols {
                if self.insert_budget == 0 || self.budget == 0 {
                    self.out.warn("The drawing has too many block copies (or objects); the rest were left out");
                    return;
                }
                self.insert_budget -= 1;
                let xf = inh.xf
                    * ocs(ins.extrusion_direction.z)
                    * Affine::translate(pt(&ins.location).to_vec2())
                    * Affine::rotate(ins.rotation.to_radians())
                    * Affine::translate((f64::from(c) * ins.column_spacing, f64::from(r) * ins.row_spacing))
                    * Affine::scale_non_uniform(sx, sy)
                    * Affine::translate(-pt(&block.base_point).to_vec2());
                let group = if self.opts.group_blocks { Some(self.out.new_group(inh.group, Some(block.name.clone()))) } else { inh.group };
                let child = Inherit {
                    xf,
                    layer: Some(layer.clone()),
                    colour: Some(colour),
                    line_type: line_type.clone().or_else(|| inh.line_type.clone()),
                    group,
                    depth: inh.depth + 1,
                };
                for be in &block.entities {
                    self.entity(be, &child);
                }
            }
        }
        // Attributes are already in drawing coordinates.
        for a in ins.attributes() {
            let ae = Entity { common: e.common.clone(), specific: EntityType::Attribute(a.clone()) };
            self.attribute(&ae, a, inh);
        }
    }

    fn dimension(&mut self, e: &Entity, block_name: &str, inh: &Inherit) {
        let Some(block) = self.blocks.get(&block_name.to_ascii_uppercase()).copied() else { return };
        if inh.depth >= MAX_DEPTH || self.insert_budget == 0 || self.budget == 0 {
            return;
        }
        self.insert_budget -= 1;
        let layer = self.effective_layer(e, inh);
        let colour = self.colour(e, &layer, inh);
        let child = Inherit {
            xf: inh.xf,
            layer: Some(layer),
            colour: Some(colour),
            line_type: inh.line_type.clone(),
            group: inh.group,
            depth: inh.depth + 1,
        };
        for be in &block.entities {
            self.entity(be, &child);
        }
    }
}

/// SOLID / TRACE outline; DXF corner order is 1-2-4-3.
fn quad_path(c: [&dxf::Point; 4]) -> BezPath {
    let mut p = BezPath::new();
    p.move_to(pt(c[0]));
    p.line_to(pt(c[1]));
    if (pt(c[3]) - pt(c[2])).hypot() > 1e-12 {
        p.line_to(pt(c[3]));
    }
    p.line_to(pt(c[2]));
    p.close_path();
    p
}

struct TextRec {
    text: String,
    style: String,
    height: f64,
    anchor: Point,
    rotation: f64,
    width_factor: f64,
    oblique: f64,
    h: HAlign,
    v: VAlign,
    ocs_z: f64,
}

/// Whether text is anchored at its first point: plain left baseline text, and Aligned and Fit
/// text, which runs from its first point to its second and is read as left aligned.
fn at_first_point(h: HorizontalTextJustification, v: VerticalTextJustification) -> bool {
    match h {
        HorizontalTextJustification::Aligned | HorizontalTextJustification::Fit => true,
        HorizontalTextJustification::Left => v == VerticalTextJustification::Baseline,
        _ => false,
    }
}

fn text_align(h: HorizontalTextJustification, v: VerticalTextJustification) -> (HAlign, VAlign) {
    // "Middle" (72=4) comes with vertical 0 and centres the text both ways.
    let v = if h == HorizontalTextJustification::Middle && v == VerticalTextJustification::Baseline { VerticalTextJustification::Middle } else { v };
    let h = match h {
        HorizontalTextJustification::Left | HorizontalTextJustification::Aligned | HorizontalTextJustification::Fit => HAlign::Left,
        HorizontalTextJustification::Center | HorizontalTextJustification::Middle => HAlign::Center,
        HorizontalTextJustification::Right => HAlign::Right,
    };
    let v = match v {
        VerticalTextJustification::Baseline => VAlign::Baseline,
        VerticalTextJustification::Bottom => VAlign::Bottom,
        VerticalTextJustification::Middle => VAlign::Middle,
        VerticalTextJustification::Top => VAlign::Top,
    };
    (h, v)
}

/// A path through bulge vertices.
pub fn bulge_path(verts: &[(Point, f64)], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    let Some(first) = verts.first() else { return p };
    if !first.0.is_finite() {
        return p;
    }
    p.move_to(first.0);
    for w in verts.windows(2) {
        bulge_to(&mut p, w[0].0, w[1].0, w[0].1);
    }
    if closed && verts.len() > 1 {
        let last = verts[verts.len() - 1];
        if (last.0 - first.0).hypot() > 1e-12 || last.1 != 0.0 {
            bulge_to(&mut p, last.0, first.0, last.1);
        }
        p.close_path();
    }
    if p.elements().len() < 2 { BezPath::new() } else { p }
}

/// Replaces `%%d`, `%%p`, `%%c`, `%%%` and drops `%%u` / `%%o` toggles.
pub fn decode_text_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && chars.peek() == Some(&'%') {
            chars.next();
            match chars.next() {
                Some('d' | 'D') => out.push('°'),
                Some('p' | 'P') => out.push('±'),
                Some('c' | 'C') => out.push('Ø'),
                Some('%') => out.push('%'),
                Some('u' | 'U' | 'o' | 'O' | 'k' | 'K') => {}
                Some(o) => {
                    out.push_str("%%");
                    out.push(o);
                }
                None => out.push_str("%%"),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Strips MTEXT inline formatting and turns paragraph breaks into new lines.
pub fn strip_mtext(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' | '}' => i += 1,
            '\\' if i + 1 < chars.len() => {
                let n = chars[i + 1];
                match n {
                    'P' | 'n' => {
                        out.push('\n');
                        i += 2;
                    }
                    '~' => {
                        out.push(' ');
                        i += 2;
                    }
                    '\\' | '{' | '}' => {
                        out.push(n);
                        i += 2;
                    }
                    'L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'N' => i += 2,
                    'S' => {
                        // Stacked fraction \S1/2; -> 1/2
                        i += 2;
                        while i < chars.len() && chars[i] != ';' {
                            let ch = chars[i];
                            out.push(if ch == '^' || ch == '#' { '/' } else { ch });
                            i += 1;
                        }
                        i += 1;
                    }
                    'U' if chars.get(i + 2) == Some(&'+') => {
                        let hex: String = chars.iter().skip(i + 3).take(4).collect();
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            out.push(ch);
                        }
                        i += 7;
                    }
                    // Codes with an argument ending in ';' (font, height, colour, width ...).
                    _ => {
                        i += 2;
                        while i < chars.len() && chars[i] != ';' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    decode_text_codes(&out)
}

/// Converts a DXF spline to Béziers. Returns the path and whether it had to be approximated.
fn spline_path(s: &dxf::entities::Spline) -> (BezPath, bool) {
    let closed = s.flags & 1 != 0;
    let degree = s.degree_of_curve.clamp(1, 11) as usize;
    let cps: Vec<(f64, f64, f64)> = s.control_points.iter().map(|p| (p.x, p.y, 1.0)).collect();
    if cps.len() < 2 {
        let fits: Vec<Point> = s.fit_points.iter().map(pt).collect();
        return (fit_point_path(&fits, closed), false);
    }
    let weights: Vec<f64> = if s.weight_values.len() == cps.len() && s.weight_values.iter().all(|w| *w > 0.0 && w.is_finite()) {
        s.weight_values.clone()
    } else {
        vec![1.0; cps.len()]
    };
    let rational = weights.iter().any(|w| (w - weights[0]).abs() > 1e-9);
    let n = cps.len();
    let p = degree.min(n - 1);
    let knots: Vec<f64> =
        if s.knot_values.len() == n + p + 1 && s.knot_values.iter().all(|k| k.is_finite()) && s.knot_values.windows(2).all(|w| w[1] >= w[0]) {
            s.knot_values.clone()
        } else {
            clamped_knots(n, p)
        };
    let hom: Vec<[f64; 3]> = cps.iter().zip(&weights).map(|(c, w)| [c.0 * w, c.1 * w, *w]).collect();
    let mut path = BezPath::new();
    let mut approx = false;
    let mut started = false;
    for k in p..n {
        let (a, b) = (knots[k], knots[k + 1]);
        if !(b > a) {
            continue;
        }
        // Bézier control points of this span via blossoming.
        let bez: Vec<[f64; 3]> = (0..=p)
            .map(|i| {
                let ts: Vec<f64> = (0..p).map(|j| if j < p - i { a } else { b }).collect();
                blossom(&hom, &knots, p, k, &ts)
            })
            .collect();
        let pts: Vec<Point> = bez.iter().map(|h| if h[2].abs() > 1e-300 { Point::new(h[0] / h[2], h[1] / h[2]) } else { Point::ZERO }).collect();
        if !started {
            path.move_to(pts[0]);
            started = true;
        }
        let equal_w = bez.iter().all(|h| (h[2] - bez[0][2]).abs() < 1e-9 * bez[0][2].abs().max(1.0));
        match (p, rational && !equal_w) {
            (1, _) => path.line_to(pts[1]),
            (2, false) => path.quad_to(pts[1], pts[2]),
            (3, false) => path.curve_to(pts[1], pts[2], pts[3]),
            _ => {
                approx = true;
                const STEPS: usize = 24;
                for st in 1..=STEPS {
                    let t = st as f64 / STEPS as f64;
                    path.line_to(eval_bezier_h(&bez, t));
                }
            }
        }
    }
    if closed && started {
        path.close_path();
    }
    (path, approx)
}

fn clamped_knots(n: usize, p: usize) -> Vec<f64> {
    let mut k = vec![0.0; p + 1];
    let inner = n.saturating_sub(p + 1);
    for i in 1..=inner {
        k.push(i as f64);
    }
    let last = (inner + 1) as f64;
    k.extend(std::iter::repeat_n(last, p + 1));
    k
}

/// Blossom of the B-spline at `ts` on span `k` (generalised de Boor).
fn blossom(cps: &[[f64; 3]], knots: &[f64], p: usize, k: usize, ts: &[f64]) -> [f64; 3] {
    let mut d: Vec<[f64; 3]> = (0..=p).map(|j| cps[k + j - p]).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k + j - p;
            let denom = knots[i + p + 1 - r] - knots[i];
            let alpha = if denom.abs() < 1e-300 { 0.0 } else { (ts[r - 1] - knots[i]) / denom };
            let prev = d[j - 1];
            for (c, v) in d[j].iter_mut().enumerate() {
                *v = (1.0 - alpha) * prev[c] + alpha * *v;
            }
        }
    }
    d[p]
}

fn eval_bezier_h(bez: &[[f64; 3]], t: f64) -> Point {
    let mut d = bez.to_vec();
    let n = d.len();
    for r in 1..n {
        for j in 0..n - r {
            let next = d[j + 1];
            for (c, v) in d[j].iter_mut().enumerate() {
                *v = (1.0 - t) * *v + t * next[c];
            }
        }
    }
    if d[0][2].abs() > 1e-300 { Point::new(d[0][0] / d[0][2], d[0][1] / d[0][2]) } else { Point::ZERO }
}

/// A smooth curve through fit points (Catmull-Rom as cubic Béziers).
fn fit_point_path(pts: &[Point], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    let n = pts.len();
    if n < 2 {
        return p;
    }
    p.move_to(pts[0]);
    let get = |i: isize| -> Point { if closed { pts[i.rem_euclid(n as isize) as usize] } else { pts[i.clamp(0, n as isize - 1) as usize] } };
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs as isize {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        p.curve_to(c1, c2, p2);
    }
    if closed {
        p.close_path();
    }
    p
}
