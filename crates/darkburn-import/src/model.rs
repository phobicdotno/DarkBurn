//! The neutral, format-independent data every importer produces and every exporter consumes.
//!
//! Geometry is in millimetres with Y pointing up, using `kurbo` types.

use std::collections::BTreeMap;

use image::DynamicImage;
use kurbo::{Affine, BezPath, Point, Rect, Shape};
use serde::{Deserialize, Serialize};

/// An 8-bit RGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const WHITE: Rgb = Rgb(255, 255, 255);

    /// Hex form without the `#`, upper case (`FF8000`).
    pub fn hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// Builds a colour from 0..=1 floats, clamping out-of-range values.
    pub fn from_unit(r: f64, g: f64, b: f64) -> Rgb {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgb(c(r), c(g), c(b))
    }

    /// Builds a colour from a packed `0xRRGGBB` value.
    pub fn from_packed(v: u32) -> Rgb {
        Rgb(((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8)
    }

    /// Squared distance in RGB space.
    pub fn distance2(self, o: Rgb) -> u32 {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2) as u32;
        d(self.0, o.0) + d(self.1, o.1) + d(self.2, o.2)
    }
}

/// Everything an importer found in one file.
#[derive(Clone, Debug, Default)]
pub struct Imported {
    /// The shapes, in drawing order (first is bottom-most).
    pub items: Vec<Item>,
    /// Group table; `Item::group` refers to `Group::id` here. Groups may nest via `parent`.
    pub groups: Vec<Group>,
    /// Layer settings carried by the file (LightBurn cut settings). Empty for plain vector formats.
    pub layer_settings: Vec<LayerSettings>,
    /// Project-level information (LightBurn projects only).
    pub project: Option<ProjectInfo>,
    /// Number of pages in the source document, for formats that have pages (PDF/AI).
    pub page_count: Option<usize>,
    /// Things the user should know about: skipped entities, approximations, unsupported features.
    pub warnings: Vec<String>,
}

impl Imported {
    /// Bounding box of every visible item, or `None` when there is nothing to measure.
    pub fn bounds(&self) -> Option<Rect> {
        bounds_of(self.items.iter().filter(|i| !i.hidden))
    }

    /// Adds a warning once (duplicates are dropped).
    pub fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.contains(&msg) {
            self.warnings.push(msg);
        }
    }

    /// Allocates a new group id.
    pub fn new_group(&mut self, parent: Option<u32>, name: Option<String>) -> u32 {
        let id = self.groups.len() as u32 + 1;
        self.groups.push(Group { id, parent, name });
        id
    }
}

/// Bounding box over a set of items.
pub fn bounds_of<'a>(items: impl Iterator<Item = &'a Item>) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for item in items {
        if let Some(b) = item.bounds() {
            out = Some(match out {
                Some(o) => o.union(b),
                None => b,
            });
        }
    }
    out
}

/// A group of items. Ids start at 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: u32,
    pub parent: Option<u32>,
    pub name: Option<String>,
}

/// One imported object.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: ItemKind,
    /// Source layer name (DXF layer, PDF/AI optional-content layer, HPGL pen, LightBurn cut setting name).
    pub layer: Option<String>,
    /// LightBurn cut-setting index this item belongs to, when known.
    pub layer_index: Option<u32>,
    /// Innermost group this item belongs to (see `Imported::groups`).
    pub group: Option<u32>,
    /// The item was hidden in the source (for example on a hidden AI layer).
    pub hidden: bool,
}

impl Item {
    pub fn new(kind: ItemKind) -> Item {
        Item { kind, layer: None, layer_index: None, group: None, hidden: false }
    }

    pub fn path(path: BezPath, stroke: Option<Rgb>, fill: Option<Rgb>) -> Item {
        Item::new(ItemKind::Path(PathItem { path, stroke, fill, dash: None }))
    }

    pub fn with_layer(mut self, layer: impl Into<String>) -> Item {
        self.layer = Some(layer.into());
        self
    }

    pub fn with_group(mut self, group: Option<u32>) -> Item {
        self.group = group;
        self
    }

    /// Bounding box in mm.
    pub fn bounds(&self) -> Option<Rect> {
        match &self.kind {
            ItemKind::Path(p) => (!p.path.is_empty()).then(|| p.path.bounding_box()),
            ItemKind::Text(t) => t.outline.as_ref().filter(|o| !o.is_empty()).map(|o| o.bounding_box()).or_else(|| {
                let o = t.transform * Point::ZERO;
                Some(Rect::from_points(o, o))
            }),
            ItemKind::Image(i) => Some(i.transform.transform_rect_bbox(Rect::new(0.0, 0.0, 1.0, 1.0))),
        }
    }

    /// Applies an affine transform (mm to mm) to the item.
    pub fn transform(&mut self, a: Affine) {
        match &mut self.kind {
            ItemKind::Path(p) => p.path.apply_affine(a),
            ItemKind::Text(t) => {
                t.transform = a * t.transform;
                if let Some(o) = &mut t.outline {
                    o.apply_affine(a);
                }
            }
            ItemKind::Image(i) => i.transform = a * i.transform,
        }
    }

    /// The main colour of the item: stroke, else fill, else text colour.
    pub fn colour(&self) -> Option<Rgb> {
        match &self.kind {
            ItemKind::Path(p) => p.stroke.or(p.fill),
            ItemKind::Text(t) => Some(t.colour),
            ItemKind::Image(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Path(PathItem),
    Text(TextItem),
    Image(ImageItem),
}

/// A vector path.
#[derive(Clone, Debug, PartialEq)]
pub struct PathItem {
    /// Geometry in mm, Y up. May hold several subpaths.
    pub path: BezPath,
    pub stroke: Option<Rgb>,
    pub fill: Option<Rgb>,
    /// Dash pattern in mm (on, off, on, off ...) when the source used one. Laser output ignores it.
    pub dash: Option<Vec<f64>>,
}

/// Horizontal text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Vertical text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VAlign {
    #[default]
    Baseline,
    Bottom,
    Middle,
    Top,
}

/// A text object. The app renders it with the named font; `outline` is a ready-made
/// outline (LightBurn backup path) to use when the font is missing.
#[derive(Clone, Debug)]
pub struct TextItem {
    pub text: String,
    /// Font family or font file name as written in the source.
    pub font: String,
    /// Source text style name (DXF STYLE), if any.
    pub style: Option<String>,
    /// Cap/text height in mm before `transform` scaling.
    pub height: f64,
    pub h_align: HAlign,
    pub v_align: VAlign,
    /// Places text space (origin at the alignment point, baseline along +X, mm) into the drawing.
    pub transform: Affine,
    pub colour: Rgb,
    /// Outline in drawing coordinates (already transformed), if the file carried one.
    pub outline: Option<BezPath>,
    /// Extra format-specific attributes (letter spacing, line spacing, bold ...).
    pub attributes: BTreeMap<String, String>,
}

/// An embedded raster image.
///
/// `transform` maps the unit square to millimetres: (0,0) is the image's bottom-left
/// corner and (1,1) its top-right corner (row 0 of the pixel data is the top row).
#[derive(Clone, Debug)]
pub struct ImageItem {
    pub image: DynamicImage,
    pub transform: Affine,
    /// Format-specific attributes (LightBurn gamma, contrast, dither mode ...).
    pub attributes: BTreeMap<String, String>,
}

impl ImageItem {
    /// The four corners in mm: bottom-left, bottom-right, top-right, top-left.
    pub fn corners(&self) -> [Point; 4] {
        let t = self.transform;
        [t * Point::new(0.0, 0.0), t * Point::new(1.0, 0.0), t * Point::new(1.0, 1.0), t * Point::new(0.0, 1.0)]
    }
}

/// A neutral layer / cut-settings record (from LightBurn `CutSetting`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerSettings {
    /// Layer index (LightBurn's `index`, 0-based).
    pub index: u32,
    pub name: String,
    /// `Cut`, `Scan`, `Image`, `Tool`, `Offset` ... as written in the source.
    pub mode: String,
    /// Every field the source carried, as key/value text (`speed`, `maxPower`, `numPasses` ...).
    pub values: BTreeMap<String, String>,
}

impl LayerSettings {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.trim().parse().ok())
    }

    /// Speed in mm/s.
    pub fn speed(&self) -> Option<f64> {
        self.get_f64("speed")
    }
    pub fn min_power(&self) -> Option<f64> {
        self.get_f64("minPower")
    }
    pub fn max_power(&self) -> Option<f64> {
        self.get_f64("maxPower")
    }
    pub fn passes(&self) -> Option<u32> {
        self.get_f64("numPasses").map(|v| v.max(0.0) as u32)
    }
    /// Scan line interval in mm.
    pub fn interval(&self) -> Option<f64> {
        self.get_f64("interval")
    }
    /// Scan angle in degrees.
    pub fn angle(&self) -> Option<f64> {
        self.get_f64("angle")
    }
    pub fn priority(&self) -> Option<i64> {
        self.get_f64("priority").map(|v| v as i64)
    }
}

/// Project-level data from a LightBurn project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectInfo {
    pub app_version: Option<String>,
    /// 0 = `.lbrn` (verbose XML), 1 = `.lbrn2` (compact).
    pub format_version: Option<u32>,
    pub material_height: Option<f64>,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub notes: Option<String>,
    pub show_notes_on_load: bool,
    /// Raw PNG bytes of the embedded thumbnail.
    pub thumbnail_png: Option<Vec<u8>>,
    /// Variable-text state (`Start`, `End`, `Current`, `Increment`, `AutoAdvance` ...).
    pub variable_text: BTreeMap<String, String>,
    /// Optimisation / planner settings (`UIPrefs` children).
    pub ui_prefs: BTreeMap<String, String>,
}
