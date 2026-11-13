//! DXF export (via the IxMilia `dxf` crate, MIT).
//!
//! Paths become LWPOLYLINEs (POLYLINE for R12) with bulge arcs or straight segments; closed
//! circles become CIRCLE entities. Layers are made per colour or per item layer.

use std::collections::BTreeMap;

use dxf::LwPolylineVertex;
use dxf::entities::{Circle, Entity, EntityType, LwPolyline, Polyline, Text, Vertex};
use dxf::enums::{AcadVersion, Units};
use dxf::tables::Layer;
use dxf::{Color, Drawing};
use kurbo::{BezPath, Point};

use crate::dxf_import::{aci_to_rgb, nearest_aci};
use crate::geom;
use crate::model::*;

/// How curves are written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CurveMode {
    /// Circular arcs as polyline bulges (other curves are fitted with arcs).
    #[default]
    Arcs,
    /// Straight segments only.
    Segments,
}

/// How layers are named.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerNaming {
    /// One layer per colour, named after its hex value.
    #[default]
    ByColour,
    /// The item's own layer name (falls back to its colour).
    ByItemLayer,
}

/// DXF file version to write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DxfVersion {
    /// AutoCAD R12 (POLYLINE entities, most widely read).
    R12,
    /// AutoCAD 2000 (LWPOLYLINE).
    #[default]
    R2000,
    /// AutoCAD 2007 (LWPOLYLINE, true colour).
    R2007,
}

#[derive(Clone, Debug)]
pub struct DxfExportOptions {
    pub curves: CurveMode,
    /// Fitting / flattening tolerance in mm.
    pub tolerance: f64,
    pub layers: LayerNaming,
    /// Round coordinates to this many decimals (`Some(2)` = 0.01 mm).
    pub decimals: Option<u32>,
    /// Write recognised circles as CIRCLE entities.
    pub circles: bool,
    pub version: DxfVersion,
}

impl Default for DxfExportOptions {
    fn default() -> Self {
        DxfExportOptions {
            curves: CurveMode::Arcs,
            tolerance: 0.01,
            layers: LayerNaming::ByColour,
            decimals: None,
            circles: true,
            version: DxfVersion::R2000,
        }
    }
}

/// The written file plus anything that could not be written.
#[derive(Clone, Debug, Default)]
pub struct DxfExport {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
}

/// DXF layer name allowed characters; others become `_`.
pub fn sanitize_layer_name(name: &str) -> String {
    let s: String =
        name.chars().map(|c| if c.is_control() || "<>/\\\":;?*|=`,".contains(c) { '_' } else { c }).collect::<String>().trim().to_string();
    if s.is_empty() { "0".into() } else { s.chars().take(255).collect() }
}

fn round(v: f64, decimals: Option<u32>) -> f64 {
    // DXF has no spelling for NaN or the infinities.
    let v = if v.is_finite() { v } else { 0.0 };
    match decimals {
        Some(d) => {
            let f = 10f64.powi(d.min(12) as i32);
            (v * f).round() / f
        }
        None => v,
    }
}

/// Writes items as DXF. Hidden items are skipped; images are not written (a warning says so).
pub fn export(items: &[Item], opts: &DxfExportOptions) -> DxfExport {
    let mut drawing = Drawing::new();
    drawing.header.version = match opts.version {
        DxfVersion::R12 => AcadVersion::R12,
        DxfVersion::R2000 => AcadVersion::R2000,
        DxfVersion::R2007 => AcadVersion::R2007,
    };
    drawing.header.default_drawing_units = Units::Millimeters;
    let mut warnings = Vec::new();
    let mut layers: BTreeMap<String, Rgb> = BTreeMap::new();
    let tol = if opts.tolerance > 0.0 { opts.tolerance } else { 0.01 };
    let r = |p: Point| dxf::Point::new(round(p.x, opts.decimals), round(p.y, opts.decimals), 0.0);
    let mut images = 0;
    let mut entities: Vec<Entity> = Vec::new();
    let mut polylines_r12: Vec<(Polyline, Vec<Vertex>, String, Rgb, Rgb)> = Vec::new();

    for item in items.iter().filter(|i| !i.hidden) {
        let colour = item.colour().unwrap_or(Rgb::BLACK);
        let layer = match (opts.layers, &item.layer) {
            (LayerNaming::ByItemLayer, Some(l)) if !l.trim().is_empty() => sanitize_layer_name(l),
            _ => format!("C_{}", colour.hex()),
        };
        let layer_colour = *layers.entry(layer.clone()).or_insert(colour);
        let path: Option<&BezPath> = match &item.kind {
            ItemKind::Path(p) => Some(&p.path),
            ItemKind::Text(t) => match &t.outline {
                Some(o) => Some(o),
                None => {
                    let origin = t.transform * Point::ZERO;
                    let x = t.transform * Point::new(1.0, 0.0) - origin;
                    let y = t.transform * Point::new(0.0, 1.0) - origin;
                    let mut text = Text { location: r(origin), value: t.text.replace("\r\n", " ").replace(['\r', '\n'], " "), ..Default::default() };
                    text.text_height = round(t.height * y.hypot(), None);
                    text.rotation = round(x.atan2().to_degrees(), None);
                    let mut e = Entity::new(EntityType::Text(text));
                    set_common(&mut e, &layer, layer_colour, colour, opts.version);
                    entities.push(e);
                    None
                }
            },
            ItemKind::Image(_) => {
                images += 1;
                None
            }
        };
        let Some(path) = path else { continue };
        for sub in geom::split_subpaths(path) {
            if opts.circles
                && let Some((c, rad)) = geom::as_circle(&sub, tol)
            {
                let mut e = Entity::new(EntityType::Circle(Circle::new(r(c), round(rad, opts.decimals))));
                set_common(&mut e, &layer, layer_colour, colour, opts.version);
                entities.push(e);
                continue;
            }
            let (verts, closed) = geom::to_bulge_polyline(&sub, tol, opts.curves == CurveMode::Arcs);
            if verts.len() < 2 {
                continue;
            }
            if opts.version == DxfVersion::R12 {
                let mut pl = Polyline::default();
                pl.set_is_closed(closed);
                let vs = verts
                    .iter()
                    .map(|(p, b)| {
                        let mut v = Vertex::new(r(*p));
                        v.bulge = *b;
                        v
                    })
                    .collect();
                polylines_r12.push((pl, vs, layer.clone(), layer_colour, colour));
            } else {
                let mut pl = LwPolyline::default();
                pl.set_is_closed(closed);
                pl.vertices = verts
                    .iter()
                    .map(|(p, b)| LwPolylineVertex { x: round(p.x, opts.decimals), y: round(p.y, opts.decimals), bulge: *b, ..Default::default() })
                    .collect();
                let mut e = Entity::new(EntityType::LwPolyline(pl));
                set_common(&mut e, &layer, layer_colour, colour, opts.version);
                entities.push(e);
            }
        }
    }
    if images > 0 {
        warnings.push(format!("{images} images were left out; DXF holds only vector shapes"));
    }
    for (name, colour) in &layers {
        let layer = Layer { name: name.clone(), color: Color::from_index(nearest_aci(*colour)), ..Default::default() };
        drawing.add_layer(layer);
    }
    for e in entities {
        drawing.add_entity(e);
    }
    for (mut pl, vs, layer, layer_colour, colour) in polylines_r12 {
        for v in vs {
            pl.add_vertex(&mut drawing, v);
        }
        let mut e = Entity::new(EntityType::Polyline(pl));
        set_common(&mut e, &layer, layer_colour, colour, opts.version);
        drawing.add_entity(e);
    }
    let mut bytes = Vec::new();
    if let Err(e) = drawing.save(&mut bytes) {
        warnings.push(format!("DXF writer failed: {e}"));
    }
    DxfExport { bytes, warnings }
}

fn set_common(e: &mut Entity, layer: &str, layer_colour: Rgb, colour: Rgb, version: DxfVersion) {
    e.common.layer = layer.to_string();
    let aci = nearest_aci(colour);
    e.common.color = Color::by_layer();
    if aci != nearest_aci(layer_colour) {
        // Another item made this layer with a different colour.
        e.common.color = Color::from_index(aci);
    }
    if version == DxfVersion::R2007 && aci_to_rgb(aci) != colour {
        e.common.color = Color::from_index(aci);
        e.common.color_24_bit = (i32::from(colour.0) << 16) | (i32::from(colour.1) << 8) | i32::from(colour.2);
    }
}
