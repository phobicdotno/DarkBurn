//! Shapes: what a design is made of. Every shape has a kind (its own geometry in local
//! coordinates), a transform that places it in the document, a layer and per-shape overrides.
//!
//! Units are millimetres, Y up, origin at the front-left corner of the work area. Primitive
//! shapes (rectangle, ellipse, polygon, image, text) are built centred on their local origin.

use kurbo::{Affine, BezPath, ParamCurveNearest, PathEl, Point, Rect, Shape as _, Vec2};
use serde::{Deserialize, Serialize};

use crate::serde_kurbo;

/// Curve flattening tolerance used to build primitive outlines, in millimetres.
pub const OUTLINE_TOLERANCE: f64 = 0.01;

/// A shape's id, unique within its document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShapeId(pub u64);

/// The direction a shape is cut in (a per shape override).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutDirection {
    /// Whatever the optimiser decides, or the drawn direction when it is off.
    #[default]
    Auto,
    /// Closed shapes clockwise.
    Clockwise,
    /// Closed shapes counter-clockwise.
    CounterClockwise,
    /// Reverse the drawn direction.
    Reversed,
}

impl CutDirection {
    pub const ALL: [CutDirection; 4] = [CutDirection::Auto, CutDirection::Clockwise, CutDirection::CounterClockwise, CutDirection::Reversed];

    pub fn label(self) -> &'static str {
        match self {
            CutDirection::Auto => "Automatic",
            CutDirection::Clockwise => "Clockwise",
            CutDirection::CounterClockwise => "Counter-clockwise",
            CutDirection::Reversed => "Reversed",
        }
    }
}

fn is_auto_direction(d: &CutDirection) -> bool {
    *d == CutDirection::Auto
}

/// A shape in the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shape {
    pub id: ShapeId,
    /// A user-given name (empty: none).
    pub name: String,
    pub kind: ShapeKind,
    /// Local to document coordinates.
    #[serde(with = "serde_kurbo::affine")]
    pub transform: Affine,
    /// Layer index: 0..30 colour layers, 30 and 31 the tool layers T1 and T2.
    pub layer: usize,
    /// Locked shapes cannot be selected, moved or edited.
    pub locked: bool,
    /// Cut order priority: lower numbers are cut first within a layer (0 is the default).
    pub priority: i32,
    /// Power scale in percent applied to this shape's layer power.
    pub power_scale: f64,
    /// Manually placed tabs (the cut skips them), as points on the outline in local coordinates.
    /// Each belongs to the sub-path nearest to it. A shape with manual tabs gets no automatic ones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<[f64; 2]>,
    /// Forced start point of the cut, a point near the outline in local coordinates. The closed
    /// sub-path nearest to it starts cutting at its point nearest to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_point: Option<[f64; 2]>,
    /// Forced start points of further sub-paths of a compound shape (same rule as `start_point`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_start_points: Vec<[f64; 2]>,
    /// Forced cut direction.
    #[serde(skip_serializing_if = "is_auto_direction")]
    pub direction: CutDirection,
    /// Set on a group that is a virtual array: the original items and the array settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub array: Option<Box<crate::varray::ArrayDef>>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            id: ShapeId(0),
            name: String::new(),
            kind: ShapeKind::Path { path: BezPath::new() },
            transform: Affine::IDENTITY,
            layer: 0,
            locked: false,
            priority: 0,
            power_scale: 100.0,
            tabs: Vec::new(),
            start_point: None,
            extra_start_points: Vec::new(),
            direction: CutDirection::Auto,
            array: None,
        }
    }
}

/// The geometry of a shape, in local coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    /// Lines and curves, open or closed, possibly several sub-paths.
    Path {
        #[serde(with = "serde_kurbo::path")]
        path: BezPath,
    },
    /// A rectangle centred on the origin. A negative radius rounds the corners inwards.
    Rect { width: f64, height: f64, radius: f64 },
    /// An ellipse centred on the origin.
    Ellipse { rx: f64, ry: f64 },
    /// A regular polygon centred on the origin, `radius` to its corners, first corner on top.
    Polygon { sides: u32, radius: f64 },
    /// Text. Its outline is the rendered text kept in `backup`, else a placeholder box.
    Text(TextShape),
    /// A QR code or barcode that stays editable. Its outline is the rendered code kept in `cache`.
    Code(CodeShape),
    /// A bitmap.
    Image(ImageShape),
    /// Shapes moved and edited together. Children keep their own transforms and layers.
    Group { children: Vec<Shape> },
}

/// Horizontal text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignX {
    #[default]
    Left,
    Middle,
    Right,
}

/// Vertical text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignY {
    #[default]
    Baseline,
    Top,
    Middle,
    Bottom,
}

/// What a text shape's content is filled from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariableText {
    #[default]
    Normal,
    DateTime,
    Merge,
    Serial,
    CutSetting,
}

/// A text shape's settings. Font rendering arrives with the text tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextShape {
    pub text: String,
    pub font: String,
    /// Capital height in millimetres.
    pub height: f64,
    pub bold: bool,
    pub italic: bool,
    pub upper_case: bool,
    pub welded: bool,
    pub distort: bool,
    pub letter_spacing: f64,
    pub line_spacing: f64,
    pub align_x: AlignX,
    pub align_y: AlignY,
    pub variable: VariableText,
    pub offset: i64,
    /// The rendered outline kept for when the font is missing, in local coordinates.
    #[serde(with = "serde_kurbo::opt_path")]
    pub backup: Option<BezPath>,
    /// Glyph outlines saved with the project so the text opens on a machine without the font.
    pub backup_glyphs: Option<darkburn_text::BackupOutline>,
    /// The text follows this path (Text on Path); `distort` bends the letters along it.
    pub on_path: Option<PathText>,
    /// What `backup` was rendered from (a hash of the settings and the evaluated text); the app
    /// renders again when it no longer matches.
    pub render_key: u64,
}

/// Where text sits on a path (local coordinates of the text shape).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathText {
    #[serde(with = "serde_kurbo::path")]
    pub path: BezPath,
    /// Distance along the path where the text starts, in millimetres.
    pub start_offset: f64,
    /// Distance from the path, positive to its left.
    pub offset: f64,
    pub center_on_path: bool,
    pub flip: bool,
}

impl Default for PathText {
    fn default() -> Self {
        PathText { path: BezPath::new(), start_offset: 0.0, offset: 0.0, center_on_path: false, flip: false }
    }
}

/// A QR code or a barcode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodeSpec {
    Qr(darkburn_text::QrSpec),
    Barcode(darkburn_text::BarcodeSpec),
}

/// A code shape's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeShape {
    pub spec: CodeSpec,
    /// The data holds variable text tokens such as `{serial}`.
    pub variable: bool,
    /// Index offset of this copy (virtual arrays).
    pub offset: i64,
    /// The rendered code, centred on the origin, in local coordinates.
    #[serde(with = "serde_kurbo::opt_path")]
    pub cache: Option<BezPath>,
    /// Why the code could not be made (too much data, bad digits), if so.
    pub error: Option<String>,
    pub render_key: u64,
}

impl Default for CodeShape {
    fn default() -> Self {
        CodeShape { spec: CodeSpec::Qr(Default::default()), variable: false, offset: 0, cache: None, error: None, render_key: 0 }
    }
}

impl Default for TextShape {
    fn default() -> Self {
        TextShape {
            text: String::new(),
            font: "IBM Plex Sans".into(),
            height: 10.0,
            bold: false,
            italic: false,
            upper_case: false,
            welded: false,
            distort: false,
            letter_spacing: 0.0,
            line_spacing: 0.0,
            align_x: AlignX::Left,
            align_y: AlignY::Baseline,
            variable: VariableText::Normal,
            offset: 0,
            backup: None,
            backup_glyphs: None,
            on_path: None,
            render_key: 0,
        }
    }
}

/// Image adjustments (Shape Properties and the Adjust Image tool).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageAdjust {
    /// -100 to 100.
    pub brightness: f64,
    /// -100 to 100.
    pub contrast: f64,
    /// 0.1 to 10, 1 is neutral.
    pub gamma: f64,
    /// Sharpening amount in percent (0 = off).
    pub enhance_amount: f64,
    /// Sharpening radius in pixels.
    pub enhance_radius: f64,
    /// Noise reduction, 0 to 10.
    pub enhance_denoise: f64,
}

impl Default for ImageAdjust {
    fn default() -> Self {
        ImageAdjust { brightness: 0.0, contrast: 0.0, gamma: 1.0, enhance_amount: 0.0, enhance_radius: 0.0, enhance_denoise: 0.0 }
    }
}

/// A bitmap placed in the design. The pixels live in the document's [`crate::ImageStore`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageShape {
    /// Key of the pixels in the document's image store.
    pub image: String,
    /// Size of the image in millimetres before the shape's transform.
    pub width: f64,
    pub height: f64,
    pub adjust: ImageAdjust,
    /// A crop path in local coordinates.
    #[serde(with = "serde_kurbo::opt_path")]
    pub crop: Option<BezPath>,
    /// The file the image came from, for Refresh Image.
    pub source: Option<String>,
    /// The crop path kept after the crop was baked into the pixels (Flatten Image Crop), in
    /// local coordinates. Remove Backup Path throws it away.
    #[serde(with = "serde_kurbo::opt_path")]
    pub backup: Option<BezPath>,
}

impl Default for ImageShape {
    fn default() -> Self {
        ImageShape { image: String::new(), width: 10.0, height: 10.0, adjust: ImageAdjust::default(), crop: None, source: None, backup: None }
    }
}

impl Shape {
    /// A new shape of `kind` at `transform` on `layer` (its id is set when added to a document).
    pub fn new(kind: ShapeKind, transform: Affine, layer: usize) -> Shape {
        Shape { kind, transform, layer, ..Shape::default() }
    }

    /// A rectangle covering `r` (document coordinates).
    pub fn rect(r: Rect, radius: f64, layer: usize) -> Shape {
        let r = r.abs();
        Shape::new(ShapeKind::Rect { width: r.width(), height: r.height(), radius }, Affine::translate(r.center().to_vec2()), layer)
    }

    /// An ellipse filling `r`.
    pub fn ellipse(r: Rect, layer: usize) -> Shape {
        let r = r.abs();
        Shape::new(ShapeKind::Ellipse { rx: r.width() / 2.0, ry: r.height() / 2.0 }, Affine::translate(r.center().to_vec2()), layer)
    }

    /// A regular polygon around `centre`.
    pub fn polygon(centre: Point, sides: u32, radius: f64, layer: usize) -> Shape {
        Shape::new(ShapeKind::Polygon { sides: sides.clamp(3, 1000), radius }, Affine::translate(centre.to_vec2()), layer)
    }

    /// A path in document coordinates.
    pub fn path(path: BezPath, layer: usize) -> Shape {
        Shape::new(ShapeKind::Path { path }, Affine::IDENTITY, layer)
    }

    /// A short name for the kind ("Rectangle", "Path", …).
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            ShapeKind::Path { .. } => "Path",
            ShapeKind::Rect { .. } => "Rectangle",
            ShapeKind::Ellipse { .. } => "Ellipse",
            ShapeKind::Polygon { .. } => "Polygon",
            ShapeKind::Text(_) => "Text",
            ShapeKind::Code(c) => match c.spec {
                CodeSpec::Qr(_) => "QR code",
                CodeSpec::Barcode(_) => "Barcode",
            },
            ShapeKind::Image(_) => "Image",
            ShapeKind::Group { .. } => "Group",
        }
    }

    /// The outline in local coordinates.
    pub fn local_outline(&self) -> BezPath {
        match &self.kind {
            ShapeKind::Path { path } => path.clone(),
            ShapeKind::Rect { width, height, radius } => rounded_rect(*width, *height, *radius),
            ShapeKind::Ellipse { rx, ry } => {
                let r = rx.abs().max(ry.abs());
                if !curve_fits(r) {
                    return Rect::new(-rx.abs(), -ry.abs(), rx.abs(), ry.abs()).to_path(0.1);
                }
                kurbo::Ellipse::new(Point::ZERO, Vec2::new(*rx, *ry), 0.0).path_elements(outline_tolerance(r)).collect()
            }
            ShapeKind::Polygon { sides, radius } => polygon(*sides, *radius),
            ShapeKind::Text(t) => t.backup.clone().unwrap_or_else(|| Rect::from_origin_size(Point::ZERO, t.placeholder_size()).to_path(0.1)),
            ShapeKind::Code(c) => c.cache.clone().unwrap_or_else(|| Rect::from_center_size(Point::ZERO, (10.0, 10.0)).to_path(0.1)),
            ShapeKind::Image(img) => img.crop.clone().unwrap_or_else(|| Rect::from_center_size(Point::ZERO, (img.width, img.height)).to_path(0.1)),
            ShapeKind::Group { children } => {
                let mut all = BezPath::new();
                for c in children {
                    all.extend(c.outline());
                }
                all
            }
        }
    }

    /// The outline in document coordinates. This is what other crates (geometry, job) read.
    pub fn outline(&self) -> BezPath {
        let mut p = self.local_outline();
        p.apply_affine(self.transform);
        p
    }

    /// Bounding box in document coordinates (empty shapes give a zero rect at their origin).
    pub fn bbox(&self) -> Rect {
        let p = self.outline();
        if p.elements().is_empty() {
            let o = self.transform * Point::ZERO;
            return Rect::from_points(o, o);
        }
        p.bounding_box()
    }

    /// Whether every sub-path is closed (open shapes cannot be filled).
    pub fn is_closed(&self) -> bool {
        match &self.kind {
            ShapeKind::Path { path } => path_is_closed(path),
            ShapeKind::Group { children } => children.iter().all(Shape::is_closed),
            _ => true,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self.kind, ShapeKind::Image(_))
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, ShapeKind::Group { .. })
    }

    /// Distance from document point `p` to the outline.
    pub fn distance_to_outline(&self, p: Point) -> f64 {
        path_distance(&self.outline(), p)
    }

    /// Whether `p` is inside the (closed) outline, by the non-zero rule.
    pub fn contains(&self, p: Point) -> bool {
        if let ShapeKind::Group { children } = &self.kind {
            let local = self.transform.inverse() * p;
            return children.iter().any(|c| c.contains(local));
        }
        self.is_closed() && self.outline().winding(p) != 0
    }

    /// This shape, or for a group every descendant, with document transforms applied.
    pub fn leaves(&self) -> Vec<Shape> {
        match &self.kind {
            ShapeKind::Group { children } => children
                .iter()
                .flat_map(|c| {
                    let mut c = c.clone();
                    c.transform = self.transform * c.transform;
                    c.leaves()
                })
                .collect(),
            _ => vec![self.clone()],
        }
    }

    /// Every forced start point of this shape, in local coordinates.
    pub fn start_points(&self) -> impl Iterator<Item = [f64; 2]> + '_ {
        self.start_point.iter().chain(&self.extra_start_points).copied()
    }

    /// Replace every forced start point (the first one becomes `start_point`).
    pub fn set_start_points(&mut self, points: &[[f64; 2]]) {
        self.start_point = points.first().copied();
        self.extra_start_points = points.iter().skip(1).copied().collect();
    }

    /// Set the layer of this shape and every child.
    pub fn set_layer(&mut self, layer: usize) {
        self.layer = layer;
        if let ShapeKind::Group { children } = &mut self.kind {
            for c in children {
                c.set_layer(layer);
            }
        }
        // A virtual array keeps its originals on the same layer, so editing it keeps the layer.
        if let Some(a) = &mut self.array {
            for s in &mut a.source {
                s.set_layer(layer);
            }
        }
    }

    /// The layers used by this shape (several for groups), sorted.
    pub fn layers(&self) -> Vec<usize> {
        match &self.kind {
            ShapeKind::Group { children } => {
                let mut v: Vec<usize> = children.iter().flat_map(Shape::layers).collect();
                v.sort_unstable();
                v.dedup();
                v
            }
            _ => vec![self.layer],
        }
    }

    /// The keys of every image this shape uses: its own, its children's, and those of a virtual
    /// array's originals (the copies are made from them again).
    pub fn image_keys(&self) -> Vec<String> {
        let mut keys = match &self.kind {
            ShapeKind::Image(img) => vec![img.image.clone()],
            ShapeKind::Group { children } => children.iter().flat_map(Shape::image_keys).collect(),
            _ => Vec::new(),
        };
        if let Some(a) = &self.array {
            for k in a.source.iter().flat_map(Shape::image_keys) {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
        }
        keys
    }

    /// Node points of the outline (end points of every segment), for snapping and editing.
    pub fn nodes(&self) -> Vec<Point> {
        self.outline()
            .elements()
            .iter()
            .filter_map(|el| match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(p),
                PathEl::ClosePath => None,
            })
            .collect()
    }
}

impl TextShape {
    /// The placeholder box size used until fonts are rendered.
    pub fn placeholder_size(&self) -> kurbo::Size {
        let lines: Vec<&str> = self.text.lines().collect();
        let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(1) as f64;
        let rows = lines.len().max(1) as f64;
        let w = longest * self.height * (0.62 + self.letter_spacing.max(-0.5) * 0.1);
        let h = self.height * (rows + (rows - 1.0) * (0.25 + self.line_spacing));
        kurbo::Size::new(w.max(0.1), h.max(0.1))
    }
}

/// Whether each sub-path of `path` ends closed (a ClosePath or ending where it began).
pub fn path_is_closed(path: &BezPath) -> bool {
    let mut start: Option<Point> = None;
    let mut last = Point::ZERO;
    let mut open = false;
    let mut any = false;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if open && start.is_some_and(|s| s.distance(last) > 1e-6) {
                    return false;
                }
                start = Some(p);
                last = p;
                open = true;
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                last = p;
                any = true;
            }
            PathEl::ClosePath => open = false,
        }
    }
    any && !(open && start.is_some_and(|s| s.distance(last) > 1e-6))
}

/// Distance from `p` to the nearest point of `path`.
pub fn path_distance(path: &BezPath, p: Point) -> f64 {
    path.segments().map(|s| s.nearest(p, 1e-3).distance_sq).fold(f64::INFINITY, f64::min).sqrt()
}

/// The tolerance for the curves of an outline with radius `r`: [`OUTLINE_TOLERANCE`] for
/// anything that fits a workshop, and never finer than a billionth of the radius, so that a
/// shape with an absurd size (a typing slip, a script) still has a short outline instead of one
/// that takes all the memory there is.
/// Whether a curve of radius `r` can be worked out at all: past this the arithmetic of the
/// curve overflows (the square of the radius is no longer a number), and the outline is the
/// box around the shape instead.
fn curve_fits(r: f64) -> bool {
    r.is_finite() && r.abs() <= 1e100
}

fn outline_tolerance(r: f64) -> f64 {
    let scaled = r.abs() * 1e-9;
    if scaled.is_finite() { OUTLINE_TOLERANCE.max(scaled) } else { OUTLINE_TOLERANCE }
}

/// A rectangle `w` by `h` centred on the origin, corners rounded by `radius` (negative rounds
/// them inwards), traced counter-clockwise from the bottom edge.
pub fn rounded_rect(w: f64, h: f64, radius: f64) -> BezPath {
    let (w, h) = (w.abs(), h.abs());
    let (x0, y0, x1, y1) = (-w / 2.0, -h / 2.0, w / 2.0, h / 2.0);
    let r = radius.abs().min(w / 2.0).min(h / 2.0);
    if r < 1e-9 || !curve_fits(r) {
        return Rect::new(x0, y0, x1, y1).to_path(0.1);
    }
    use std::f64::consts::FRAC_PI_2 as Q;
    let inward = radius < 0.0;
    // Corner centres, start angles and sweeps: bottom-right, top-right, top-left, bottom-left.
    let corners: [(Point, f64, f64); 4] = if inward {
        [(Point::new(x1, y0), 2.0 * Q, -Q), (Point::new(x1, y1), 3.0 * Q, -Q), (Point::new(x0, y1), 0.0, -Q), (Point::new(x0, y0), Q, -Q)]
    } else {
        [
            (Point::new(x1 - r, y0 + r), -Q, Q),
            (Point::new(x1 - r, y1 - r), 0.0, Q),
            (Point::new(x0 + r, y1 - r), Q, Q),
            (Point::new(x0 + r, y0 + r), 2.0 * Q, Q),
        ]
    };
    let mut p = BezPath::new();
    p.move_to((x0 + r, y0));
    for (centre, start, sweep) in corners {
        let arc = kurbo::Arc::new(centre, (r, r), start, sweep, 0.0);
        p.line_to(centre + Vec2::from_angle(start) * r);
        p.extend(arc.append_iter(outline_tolerance(r)));
    }
    p.close_path();
    p
}

/// A regular polygon with `sides` corners on a circle of `radius`, the first one on top.
pub fn polygon(sides: u32, radius: f64) -> BezPath {
    let n = sides.clamp(3, 1000);
    let mut p = BezPath::new();
    for i in 0..n {
        let a = std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::TAU / n as f64;
        let v = Point::new(radius * a.cos(), radius * a.sin());
        if i == 0 {
            p.move_to(v);
        } else {
            p.line_to(v);
        }
    }
    p.close_path();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn rectangle_outline_and_bbox() {
        let s = Shape::rect(Rect::new(10.0, 20.0, 50.0, 40.0), 0.0, 3);
        let b = s.bbox();
        assert!(near(b.x0, 10.0) && near(b.y0, 20.0) && near(b.x1, 50.0) && near(b.y1, 40.0));
        assert!(s.is_closed());
        assert!(s.contains(Point::new(30.0, 30.0)));
        assert!(!s.contains(Point::new(5.0, 30.0)));
        assert_eq!(s.layer, 3);
    }

    #[test]
    fn a_huge_ellipse_or_rounded_rectangle_has_a_short_outline() {
        // Found by the monkey (seed 1000, step 48): an ellipse 1e150 mm wide asked for 120 GB.
        let e = Shape::new(ShapeKind::Ellipse { rx: 5e149, ry: 3.9 }, Affine::IDENTITY, 0);
        assert!(e.local_outline().elements().len() < 200, "{} elements", e.local_outline().elements().len());
        let r = rounded_rect(1e150, 1e150, 4e149);
        assert!(r.elements().len() < 200, "{} elements", r.elements().len());
        // Shapes of a sensible size keep the outline they had.
        let e = Shape::new(ShapeKind::Ellipse { rx: 500.0, ry: 300.0 }, Affine::IDENTITY, 0);
        let exact: BezPath = kurbo::Ellipse::new(Point::ZERO, Vec2::new(500.0, 300.0), 0.0).path_elements(OUTLINE_TOLERANCE).collect();
        assert_eq!(e.local_outline(), exact);
    }

    #[test]
    fn an_ellipse_near_the_largest_number_there_is_still_has_an_outline() {
        // Found by the monkey (seed 3026, step 26): an ellipse 1.7e308 mm high asked for 120 GB,
        // because the arithmetic of the curve overflows at that size.
        let e = Shape::new(ShapeKind::Ellipse { rx: 99.8, ry: 8.5e307 }, Affine::IDENTITY, 0);
        assert!(e.local_outline().elements().len() < 200, "{} elements", e.local_outline().elements().len());
        let e = Shape::new(ShapeKind::Ellipse { rx: f64::INFINITY, ry: f64::NAN }, Affine::IDENTITY, 0);
        assert!(e.local_outline().elements().len() < 200);
        let r = rounded_rect(1.7e308, 1.7e308, 8e307);
        assert!(r.elements().len() < 200, "{} elements", r.elements().len());
    }

    #[test]
    fn rounded_corners_go_out_and_in() {
        let out = rounded_rect(20.0, 10.0, 3.0);
        let inn = rounded_rect(20.0, 10.0, -3.0);
        for p in [&out, &inn] {
            let b = p.bounding_box();
            assert!(near(b.width(), 20.0) && near(b.height(), 10.0), "{b:?}");
        }
        assert_eq!(out.winding(Point::new(9.9, 4.9)), 0, "a round corner cuts the tip");
        assert_eq!(inn.winding(Point::new(9.9, 4.9)), 0);
        assert_ne!(out.winding(Point::new(8.5, 3.5)), 0);
        assert_eq!(inn.winding(Point::new(9.0, 4.0)), 0, "an inward corner cuts a bite");
        assert_ne!(inn.winding(Point::new(9.5, 0.0)), 0);
        assert!(inn.area().abs() < out.area().abs());
        let pill = rounded_rect(20.0, 10.0, 50.0);
        assert!(near(pill.bounding_box().height(), 10.0), "radius clamped to half the short side");
    }

    #[test]
    fn polygon_has_n_corners_on_its_circle() {
        let p = polygon(6, 10.0);
        let s = Shape::polygon(Point::new(5.0, 5.0), 6, 10.0, 0);
        assert_eq!(s.nodes().len(), 6);
        for el in p.elements() {
            if let PathEl::LineTo(v) | PathEl::MoveTo(v) = el {
                assert!(near(v.to_vec2().hypot(), 10.0));
            }
        }
        assert!(s.nodes()[0].y > 14.9, "first corner on top");
    }

    #[test]
    fn ellipse_and_transform() {
        let mut s = Shape::ellipse(Rect::new(0.0, 0.0, 40.0, 20.0), 0);
        let b = s.bbox();
        assert!((b.width() - 40.0).abs() < 0.05 && (b.height() - 20.0).abs() < 0.05);
        s.transform = Affine::rotate(std::f64::consts::FRAC_PI_2) * s.transform;
        let b = s.bbox();
        assert!((b.width() - 20.0).abs() < 0.05 && (b.height() - 40.0).abs() < 0.05);
    }

    #[test]
    fn open_and_closed_paths() {
        let mut open = BezPath::new();
        open.move_to((0.0, 0.0));
        open.line_to((10.0, 0.0));
        let s = Shape::path(open.clone(), 0);
        assert!(!s.is_closed());
        assert!(!s.contains(Point::new(5.0, 0.0)));
        assert!(near(s.distance_to_outline(Point::new(5.0, 3.0)), 3.0));
        open.line_to((10.0, 10.0));
        open.line_to((0.0, 0.0));
        assert!(path_is_closed(&open), "ending where it began counts as closed");
        let mut two = BezPath::new();
        two.move_to((0.0, 0.0));
        two.line_to((1.0, 0.0));
        two.close_path();
        two.move_to((5.0, 5.0));
        two.line_to((6.0, 5.0));
        assert!(!path_is_closed(&two));
        assert!(!path_is_closed(&BezPath::new()));
    }

    #[test]
    fn groups_carry_their_children() {
        let a = Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 1);
        let b = Shape::rect(Rect::new(20.0, 0.0, 30.0, 10.0), 0.0, 2);
        let mut g = Shape::new(ShapeKind::Group { children: vec![a, b] }, Affine::translate((5.0, 5.0)), 1);
        let bb = g.bbox();
        assert!(near(bb.x0, 5.0) && near(bb.x1, 35.0));
        assert_eq!(g.layers(), vec![1, 2]);
        assert_eq!(g.leaves().len(), 2);
        assert!(near(g.leaves()[1].bbox().x0, 25.0));
        assert!(g.contains(Point::new(7.0, 7.0)));
        assert!(!g.contains(Point::new(20.0, 7.0)));
        g.set_layer(4);
        assert_eq!(g.layers(), vec![4]);
    }

    #[test]
    fn text_placeholder_grows_with_text() {
        let mut t = TextShape { text: "Hi".into(), ..Default::default() };
        let small = t.placeholder_size();
        t.text = "Hello there\nsecond".into();
        let big = t.placeholder_size();
        assert!(big.width > small.width && big.height > small.height);
    }
}
