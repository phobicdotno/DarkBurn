//! File export for the Gweike M3 Ultra's vendor software (Mlaser, a rebrand of Au3tech NexCut).
//!
//! Mlaser imports DXF (its only "standard file"), G-code (`.nc/.cnc/.g/.txt`), PLT and its own
//! `.chf` craft files. DarkBurn hands it geometry only:
//!
//! * [`export_dxf_per_material`] (MCC-01): one DXF layer per material, coordinates rounded to
//!   0.01 mm, circles as CIRCLE, arcs as bulges, R12 by default for the widest reader support.
//! * [`export_nc`] (MCC-03): ISO-style G-code (`G21 G90`, `G00` rapids, `G01` lines, `G02/G03`
//!   arcs) with the cut on/off codes configurable.
//!
//! Neither file carries laser power, gas, pierce, focus or height-follower commands: the
//! machine software applies its own craft settings per layer, so its follower, piercing and gas
//! logic stay in charge (MCC-07). Nothing here opens a connection to any machine.
//!
//! # Craft files (`.chf`, MCC-02) are not written
//!
//! The research only records that `.chf` is a binary "Craft file" with CP936 (GBK) strings and
//! that a round-trip reader/writer exists in phobicMlaserLinux. Missing before a writer can be
//! built: the file header and version fields, the record layout and field order, how geometry is
//! stored (units, arc encoding, contour flags), how layers/crafts link to contours, the craft
//! parameter block (power, frequency, gas, pierce stages, follower height), checksums if any,
//! and sample files to test against. Port these from phobicMlaserLinux's format notes
//! (`docs/analysis/11-static/A9`) once they are reviewed.
//!
//! # G-code dialect caveat
//!
//! The research lists the extensions Mlaser imports but not its exact dialect. The defaults
//! (`M07` cut on, `M08` cut off, as used by common fibre cutting CNCs) must be checked against
//! Mlaser by the owner before relying on them; both are options.

use std::collections::BTreeMap;

use kurbo::Point;

use crate::dxf_export::{self, CurveMode, DxfExport, DxfExportOptions, DxfVersion, LayerNaming};
use crate::geom::{self, fmt_num};
use crate::model::*;

/// Options for the Mlaser DXF.
#[derive(Clone, Debug)]
pub struct GweikeDxfOptions {
    /// Material name per source layer name or per colour written as `#RRGGBB`.
    pub materials: BTreeMap<String, String>,
    /// Material for items that match nothing; `None` keeps the item's own layer or colour.
    pub default_material: Option<String>,
    pub version: DxfVersion,
    pub curves: CurveMode,
}

impl Default for GweikeDxfOptions {
    fn default() -> Self {
        GweikeDxfOptions { materials: BTreeMap::new(), default_material: None, version: DxfVersion::R12, curves: CurveMode::Arcs }
    }
}

/// The material an item is assigned to.
pub fn material_of(item: &Item, opts: &GweikeDxfOptions) -> String {
    if let Some(l) = &item.layer
        && let Some(m) = opts.materials.get(l)
    {
        return m.clone();
    }
    if let Some(c) = item.colour()
        && let Some(m) = opts.materials.get(&format!("#{}", c.hex()))
    {
        return m.clone();
    }
    if let Some(m) = &opts.default_material {
        return m.clone();
    }
    item.layer.clone().unwrap_or_else(|| format!("C_{}", item.colour().unwrap_or(Rgb::BLACK).hex()))
}

/// DXF with one layer per material, ready for Mlaser (MCC-01).
pub fn export_dxf_per_material(items: &[Item], opts: &GweikeDxfOptions) -> DxfExport {
    let renamed: Vec<Item> = items
        .iter()
        .map(|i| {
            let mut c = i.clone();
            c.layer = Some(material_of(i, opts));
            c
        })
        .collect();
    let dopts = DxfExportOptions {
        curves: opts.curves,
        tolerance: 0.01,
        layers: LayerNaming::ByItemLayer,
        decimals: Some(2),
        circles: true,
        version: opts.version,
    };
    dxf_export::export(&renamed, &dopts)
}

/// Options for the Mlaser G-code.
#[derive(Clone, Debug)]
pub struct NcOptions {
    /// Code that starts cutting a contour (the machine applies its own pierce routine).
    pub cut_on: String,
    /// Code that ends a contour.
    pub cut_off: String,
    /// Cutting feed in mm/min, written on `G01/G02/G03` when set. Usually left to the machine.
    pub feed: Option<f64>,
    /// Write arcs as `G02/G03`; otherwise everything is `G01` segments.
    pub arcs: bool,
    /// Fitting / flattening tolerance in mm.
    pub tolerance: f64,
    /// Decimals for coordinates.
    pub decimals: usize,
    /// Optional material per layer (same rules as the DXF export); written as comments.
    pub materials: GweikeDxfOptions,
}

impl Default for NcOptions {
    fn default() -> Self {
        NcOptions {
            cut_on: "M07".into(),
            cut_off: "M08".into(),
            feed: None,
            arcs: true,
            tolerance: 0.01,
            decimals: 3,
            materials: GweikeDxfOptions::default(),
        }
    }
}

/// G-code for Mlaser's G-code import (MCC-03). Contours are grouped per material in order of
/// first use. Images are left out.
pub fn export_nc(items: &[Item], opts: &NcOptions) -> String {
    let d = opts.decimals;
    let f = |v: f64| fmt_num(v, d);
    let mut order: Vec<String> = Vec::new();
    let mut by_mat: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
    for item in items.iter().filter(|i| !i.hidden) {
        let m = material_of(item, &opts.materials);
        if !by_mat.contains_key(&m) {
            order.push(m.clone());
        }
        by_mat.entry(m).or_default().push(item);
    }
    let mut s = String::new();
    s.push_str("(DarkBurn contour export - geometry only, the machine applies its own craft settings)\n");
    s.push_str("G21\nG90\n");
    let feed = opts.feed.filter(|v| *v > 0.0).map(|v| format!(" F{}", fmt_num(v, 1))).unwrap_or_default();
    let mut cur = Point::new(f64::NAN, f64::NAN);
    for m in &order {
        s.push_str(&format!("(MATERIAL: {})\n", m.replace(['(', ')'], "")));
        for item in &by_mat[m] {
            let path = match &item.kind {
                ItemKind::Path(p) => &p.path,
                ItemKind::Text(t) => match &t.outline {
                    Some(o) => o,
                    None => continue,
                },
                ItemKind::Image(_) => continue,
            };
            for sub in geom::split_subpaths(path) {
                let (verts, closed) = geom::to_bulge_polyline(&sub, opts.tolerance, opts.arcs);
                if verts.len() < 2 {
                    continue;
                }
                let start = verts[0].0;
                if (start - cur).hypot() > 1e-9 || cur.x.is_nan() {
                    s.push_str(&format!("G00 X{} Y{}\n", f(start.x), f(start.y)));
                }
                s.push_str(&opts.cut_on);
                s.push('\n');
                let mut segs: Vec<(Point, Point, f64)> = verts.windows(2).map(|w| (w[0].0, w[1].0, w[0].1)).collect();
                if closed {
                    let last = verts[verts.len() - 1];
                    segs.push((last.0, start, last.1));
                }
                let mut first_move = true;
                for (a, b, bulge) in segs {
                    let fd = if first_move { feed.as_str() } else { "" };
                    first_move = false;
                    if bulge.abs() > 1e-9 {
                        let chord = b - a;
                        let dist = chord.hypot();
                        let left = kurbo::Vec2::new(-chord.y, chord.x) / dist;
                        let h = dist * (1.0 - bulge * bulge) / (4.0 * bulge);
                        let center = a.midpoint(b) + left * h;
                        let code = if bulge > 0.0 { "G03" } else { "G02" };
                        s.push_str(&format!("{code} X{} Y{} I{} J{}{fd}\n", f(b.x), f(b.y), f(center.x - a.x), f(center.y - a.y)));
                    } else {
                        s.push_str(&format!("G01 X{} Y{}{fd}\n", f(b.x), f(b.y)));
                    }
                    cur = b;
                }
                s.push_str(&opts.cut_off);
                s.push('\n');
            }
        }
    }
    s.push_str("M30\n");
    s
}
