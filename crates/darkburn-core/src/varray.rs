//! Virtual arrays: a group of copies that remembers the original item(s) and the array
//! settings, so the array stays editable until it is flattened. The group's children are the
//! generated copies (so drawing, selection and output treat it as an ordinary group);
//! [`regenerate`] rebuilds them from the definition.

use darkburn_geom::array::{self, AlongMode, AlongOptions, CircularOptions, GridOptions, Shift, Spacing};
use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use serde::{Deserialize, Serialize};

use crate::serde_kurbo;
use crate::shape::{Shape, ShapeKind, VariableText};

/// Offsetting of alternate rows or columns in a grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum GridShift {
    #[default]
    None,
    /// Odd rows move sideways by this many mm.
    AltRows(f64),
    /// Odd columns move up by this many mm.
    AltCols(f64),
    /// Hexagonal layout.
    Hex,
}

/// Grid array settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridDef {
    pub rows: usize,
    pub cols: usize,
    /// Spacing is the distance between the centres of neighbours; else the gap between them.
    pub centre_to_centre: bool,
    pub dx: f64,
    pub dy: f64,
    pub shift: GridShift,
    pub mirror_alt_rows: bool,
    pub mirror_alt_cols: bool,
    pub reverse_x: bool,
    pub reverse_y: bool,
}

impl Default for GridDef {
    fn default() -> Self {
        GridDef {
            rows: 2,
            cols: 2,
            centre_to_centre: false,
            dx: 2.0,
            dy: 2.0,
            shift: GridShift::None,
            mirror_alt_rows: false,
            mirror_alt_cols: false,
            reverse_x: false,
            reverse_y: false,
        }
    }
}

impl GridDef {
    pub fn options(&self) -> GridOptions {
        let v = Vec2::new(self.dx, self.dy);
        GridOptions {
            rows: self.rows.max(1),
            cols: self.cols.max(1),
            spacing: if self.centre_to_centre { Spacing::CenterToCenter(v) } else { Spacing::Gap(v) },
            shift: match self.shift {
                GridShift::None => Shift::None,
                GridShift::AltRows(d) => Shift::AltRows(d),
                GridShift::AltCols(d) => Shift::AltCols(d),
                GridShift::Hex => Shift::HexRows,
            },
            mirror_alt_rows: self.mirror_alt_rows,
            mirror_alt_cols: self.mirror_alt_cols,
            reverse_x: self.reverse_x,
            reverse_y: self.reverse_y,
        }
    }
}

/// Circular array settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CircularDef {
    pub cx: f64,
    pub cy: f64,
    pub count: usize,
    pub start_angle: f64,
    pub sweep: f64,
    pub rotate_items: bool,
    pub clockwise: bool,
}

impl Default for CircularDef {
    fn default() -> Self {
        CircularDef { cx: 0.0, cy: 0.0, count: 6, start_angle: 0.0, sweep: 360.0, rotate_items: true, clockwise: false }
    }
}

impl CircularDef {
    pub fn options(&self) -> CircularOptions {
        CircularOptions {
            center: Point::new(self.cx, self.cy),
            count: self.count.max(1),
            start_angle_deg: self.start_angle,
            sweep_deg: self.sweep,
            rotate_items: self.rotate_items,
            clockwise: self.clockwise,
        }
    }
}

/// Copy-along-path settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlongDef {
    /// The path the copies follow (in the array's own coordinates).
    #[serde(with = "serde_kurbo::path")]
    pub path: BezPath,
    /// This many copies over the length; used when `spacing` is zero.
    pub count: usize,
    /// A copy every this many mm (0: use `count`).
    pub spacing: f64,
    pub rotate_items: bool,
    pub start_at_nearest: bool,
}

impl Default for AlongDef {
    fn default() -> Self {
        AlongDef { path: BezPath::new(), count: 5, spacing: 0.0, rotate_items: true, start_at_nearest: false }
    }
}

impl AlongDef {
    pub fn options(&self) -> AlongOptions {
        AlongOptions {
            mode: if self.spacing > 1e-9 { AlongMode::Spacing(self.spacing) } else { AlongMode::Count(self.count.max(1)) },
            rotate_items: self.rotate_items,
            start_at_nearest: self.start_at_nearest,
        }
    }
}

/// Which kind of array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArrayKind {
    Grid(GridDef),
    Circular(CircularDef),
    AlongPath(AlongDef),
}

/// The definition of a virtual array.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrayDef {
    /// The original items, in the array group's own coordinates.
    pub source: Vec<Shape>,
    pub kind: ArrayKind,
}

/// Bounding box of `shapes`.
pub fn bbox_of(shapes: &[Shape]) -> Option<Rect> {
    shapes.iter().map(Shape::bbox).reduce(|a, b| a.union(b))
}

impl ArrayDef {
    /// One transform per copy (copy 0 first).
    pub fn transforms(&self) -> Vec<Affine> {
        let Some(bb) = bbox_of(&self.source) else { return Vec::new() };
        match &self.kind {
            ArrayKind::Grid(g) => array::grid_array(bb, &g.options()),
            ArrayKind::Circular(c) => array::circular_array(bb.center(), &c.options()),
            ArrayKind::AlongPath(a) => array::copy_along_path(bb.center(), &a.path, &a.options()),
        }
    }

    /// The copies, flat: every source item under every transform, serial text advanced.
    pub fn copies(&self) -> Vec<Shape> {
        let mut out = Vec::new();
        for (i, a) in self.transforms().into_iter().enumerate() {
            for s in &self.source {
                let mut c = s.clone();
                c.transform = a * c.transform;
                c.array = None;
                advance_text(&mut c, i as i64);
                out.push(c);
            }
        }
        out
    }
}

pub fn advance_text(s: &mut Shape, n: i64) {
    match &mut s.kind {
        ShapeKind::Text(t) if t.variable != VariableText::Normal => t.offset += n,
        ShapeKind::Code(c) if c.variable => c.offset += n,
        ShapeKind::Group { children } => children.iter_mut().for_each(|c| advance_text(c, n)),
        _ => {}
    }
}

/// Rebuild an array group's children from its definition (no-op for other shapes).
/// Children get ids from `alloc`.
pub fn regenerate(shape: &mut Shape, mut alloc: impl FnMut() -> crate::ShapeId) {
    let Some(def) = shape.array.as_deref() else { return };
    let mut copies = def.copies();
    for c in &mut copies {
        renumber(c, &mut alloc);
    }
    shape.kind = ShapeKind::Group { children: copies };
}

fn renumber(s: &mut Shape, alloc: &mut impl FnMut() -> crate::ShapeId) {
    s.id = alloc();
    if let ShapeKind::Group { children } = &mut s.kind {
        children.iter_mut().for_each(|c| renumber(c, alloc));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Shape {
        Shape::rect(Rect::new(0.0, 0.0, 10.0, 10.0), 0.0, 0)
    }

    #[test]
    fn grid_copies_fill_rows_and_columns() {
        let def = ArrayDef { source: vec![square()], kind: ArrayKind::Grid(GridDef { rows: 2, cols: 3, dx: 5.0, dy: 5.0, ..Default::default() }) };
        let c = def.copies();
        assert_eq!(c.len(), 6);
        let bb = bbox_of(&c).unwrap();
        assert!((bb.width() - 40.0).abs() < 1e-6 && (bb.height() - 25.0).abs() < 1e-6, "{bb:?}");
    }

    #[test]
    fn circular_and_json_round_trip() {
        let def = ArrayDef {
            source: vec![Shape::rect(Rect::new(20.0, -2.0, 24.0, 2.0), 0.0, 0)],
            kind: ArrayKind::Circular(CircularDef { count: 4, ..Default::default() }),
        };
        assert_eq!(def.copies().len(), 4);
        let json = serde_json::to_string(&def).unwrap();
        let back: ArrayDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.copies().len(), 4);
        let along = ArrayKind::AlongPath(AlongDef { path: BezPath::from_svg("M0 0 L100 0").unwrap(), count: 3, ..Default::default() });
        let json = serde_json::to_string(&along).unwrap();
        assert!(serde_json::from_str::<ArrayKind>(&json).is_ok());
    }
}
