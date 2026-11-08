//! The test tools that draw into the document: Focus Test, Interval Test, Feeder windows,
//! Centre Finder marks and the camera alignment targets. Each puts ordinary layers and shapes
//! in the document (one undo step when the caller wraps it in an editor edit), so the result
//! runs on any device. Nothing here talks to a machine.

use darkburn_core::kurbo::{Affine, BezPath, Circle, Point, Rect, Shape as _};
use darkburn_core::layer::{self, Mode};
use darkburn_core::{Document, Shape, ShapeId};
use darkburn_vision::feeder::{self, FeedWindow};
use darkburn_vision::testpatterns::{self, FocusTestParams, IntervalTestParams};

/// What a generator added.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub shapes: Vec<ShapeId>,
    pub layers: Vec<usize>,
    pub bounds: Rect,
}

fn moved(path: &BezPath, at: Point) -> BezPath {
    let mut p = path.clone();
    p.apply_affine(Affine::translate(at.to_vec2()));
    p
}

fn named(path: BezPath, layer: usize, name: String) -> Shape {
    let mut s = Shape::path(path, layer);
    s.name = name;
    s
}

fn setup_layer(doc: &mut Document, index: usize, name: String, mode: Mode, f: impl FnOnce(&mut layer::CutSetting)) {
    let l = doc.layer_mut(index);
    l.output = true;
    l.name = name;
    l.settings.truncate(1);
    let s = l.main_mut();
    s.mode = mode;
    f(s);
    s.sanitise();
}

fn check_layers(first: usize, count: usize, label_layer: usize, labels: bool) -> Result<Vec<usize>, String> {
    let layers: Vec<usize> = (first..layer::COLOUR_LAYERS).filter(|l| !(labels && *l == label_layer)).take(count).collect();
    if layers.len() < count {
        return Err(format!("{count} rows need {count} free colour layers from layer {first}; use fewer steps or an earlier first layer."));
    }
    if label_layer >= layer::COLOUR_LAYERS {
        return Err("The label layer must be a colour layer.".into());
    }
    Ok(layers)
}

fn label_layer_setup(doc: &mut Document, index: usize) {
    setup_layer(doc, index, "Test labels".into(), Mode::Line, |s| {
        s.speed = 150.0;
        s.max_power = 10.0;
        s.passes = 1;
    });
}

/// Focus test: one line per Z offset, each on its own layer carrying that offset, with the Z
/// written beside it. The lines start at `at` (bottom left).
pub fn focus(doc: &mut Document, p: &FocusTestParams, at: Point, first_layer: usize, label_layer: usize) -> Result<Generated, String> {
    let t = testpatterns::focus_test(p).map_err(|e| e.to_string())?;
    let layers = check_layers(first_layer, t.rows.len(), label_layer, p.with_labels)?;
    let mut shapes = Vec::new();
    for (row, li) in t.rows.iter().zip(&layers) {
        setup_layer(doc, *li, format!("Focus {}", row.label), Mode::Line, |s| s.z_offset = row.z_mm);
        shapes.push(doc.add(named(moved(&row.line, at), *li, format!("Focus line {}", row.label))));
    }
    let mut used = layers;
    if p.with_labels {
        label_layer_setup(doc, label_layer);
        for row in &t.rows {
            shapes.push(doc.add(named(moved(&row.label_path, at), label_layer, format!("Label {}", row.label))));
        }
        used.push(label_layer);
    }
    let b = t.bounds;
    Ok(Generated { shapes, layers: used, bounds: Rect::new(b.x0 + at.x, b.y0 + at.y, b.x1 + at.x, b.y1 + at.y) })
}

/// Interval test: one filled block per line interval, each on its own layer with that interval.
pub fn interval(doc: &mut Document, p: &IntervalTestParams, at: Point, first_layer: usize, label_layer: usize) -> Result<Generated, String> {
    let t = testpatterns::interval_test(p).map_err(|e| e.to_string())?;
    let layers = check_layers(first_layer, t.rows.len(), label_layer, p.with_labels)?;
    let mut shapes = Vec::new();
    for (row, li) in t.rows.iter().zip(&layers) {
        setup_layer(doc, *li, format!("Interval {}", trim(row.interval_mm)), Mode::Fill, |s| s.interval = row.interval_mm);
        shapes.push(doc.add(named(moved(&row.block, at), *li, format!("Interval block {}", trim(row.interval_mm)))));
    }
    let mut used = layers;
    if p.with_labels {
        label_layer_setup(doc, label_layer);
        for row in &t.rows {
            shapes.push(doc.add(named(moved(&row.label_path, at), label_layer, format!("Label {}", trim(row.interval_mm)))));
        }
        used.push(label_layer);
    }
    let b = t.bounds;
    Ok(Generated { shapes, layers: used, bounds: Rect::new(b.x0 + at.x, b.y0 + at.y, b.x1 + at.x, b.y1 + at.y) })
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Feeder windows: an outline per window of a long piece, labelled with its number, so the
/// plan can be checked against the material. `width` is the piece width in mm.
pub fn feeder_windows(doc: &mut Document, windows: &[FeedWindow], window_height: f64, width: f64, layer_index: usize) -> Result<Generated, String> {
    if !(width > 0.0) || windows.is_empty() {
        return Err("Window width must be above zero.".into());
    }
    if layer_index >= layer::COLOUR_LAYERS {
        return Err("The layer must be a colour layer.".into());
    }
    setup_layer(doc, layer_index, "Feeder windows".into(), Mode::Line, |_| {});
    let mut shapes = Vec::new();
    let mut bounds: Option<Rect> = None;
    for w in windows {
        let r = Rect::new(0.0, w.start, width, w.start + window_height);
        shapes.push(doc.add(named(r.to_path(0.1), layer_index, format!("Feeder window {}", w.index + 1))));
        let text = format!("{}", w.index + 1);
        let h = (width / 10.0).clamp(2.0, 6.0);
        let label = darkburn_vision::strokefont::text_path(&text, h, Point::new(2.0, w.start + 2.0));
        shapes.push(doc.add(named(label, layer_index, format!("Feeder label {text}"))));
        bounds = Some(bounds.map_or(r, |b| b.union(r)));
    }
    Ok(Generated { shapes, layers: vec![layer_index], bounds: bounds.unwrap_or_default() })
}

/// A crosshair inside a ring, centred on `c`: the mark used for camera alignment targets and
/// for the centre finder's result.
pub fn target_mark(c: Point, size: f64) -> BezPath {
    let r = size / 2.0;
    let mut p = Circle::new(c, r * 0.6).to_path(0.05);
    p.move_to(Point::new(c.x - r, c.y));
    p.line_to(Point::new(c.x + r, c.y));
    p.move_to(Point::new(c.x, c.y - r));
    p.line_to(Point::new(c.x, c.y + r));
    p
}

/// Camera alignment targets: one mark per position, all on `layer_index`, for the user to cut
/// or mark on the bed and then click in the captured image.
pub fn alignment_targets(doc: &mut Document, positions: &[Point], size: f64, layer_index: usize) -> Result<Generated, String> {
    if !(size > 0.0) || positions.is_empty() {
        return Err("Target size must be above zero.".into());
    }
    if layer_index >= layer::COLOUR_LAYERS {
        return Err("The layer must be a colour layer.".into());
    }
    setup_layer(doc, layer_index, "Camera alignment targets".into(), Mode::Line, |_| {});
    let mut shapes = Vec::new();
    let mut bounds: Option<Rect> = None;
    for (i, c) in positions.iter().enumerate() {
        let path = target_mark(*c, size);
        let b = path.bounding_box();
        bounds = Some(bounds.map_or(b, |a| a.union(b)));
        shapes.push(doc.add(named(path, layer_index, format!("Alignment target {}", i + 1))));
    }
    Ok(Generated { shapes, layers: vec![layer_index], bounds: bounds.unwrap_or_default() })
}

/// The plan for a long piece, also checking the feed numbers are sane.
pub fn feeder_plan(total: f64, window_height: f64, overlap: f64) -> Result<Vec<FeedWindow>, String> {
    feeder::plan_windows(total, window_height, overlap).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_puts_one_layer_per_row_with_its_z_offset() {
        let mut d = Document::new();
        let p = FocusTestParams { steps: 5, z_start: -2.0, z_end: 2.0, ..Default::default() };
        let g = focus(&mut d, &p, Point::new(10.0, 20.0), 0, 29).unwrap();
        assert_eq!(g.layers.len(), 6, "five rows and the label layer");
        assert_eq!(d.layer(0).main().z_offset, -2.0);
        assert_eq!(d.layer(4).main().z_offset, 2.0);
        assert_eq!(g.shapes.len(), 10);
        assert!(g.bounds.x0 >= 10.0 && g.bounds.y0 >= 20.0);
        for id in &g.shapes {
            assert!(d.get(*id).unwrap().bbox().x0 >= 10.0 - 1e-9);
        }
    }

    #[test]
    fn interval_sets_each_layer_interval() {
        let mut d = Document::new();
        let p = IntervalTestParams { steps: 3, interval_start: 0.05, interval_end: 0.15, with_labels: false, ..Default::default() };
        let g = interval(&mut d, &p, Point::ZERO, 2, 29).unwrap();
        assert_eq!(g.layers, vec![2, 3, 4]);
        assert!((d.layer(3).main().interval - 0.1).abs() < 1e-9);
        assert_eq!(d.layer(3).main().mode, Mode::Fill);
    }

    #[test]
    fn too_many_rows_for_the_layers_is_refused() {
        let mut d = Document::new();
        let p = FocusTestParams { steps: 20, ..Default::default() };
        assert!(focus(&mut d, &p, Point::ZERO, 20, 29).is_err());
        assert_eq!(d.iter().count(), 0, "nothing was added");
    }

    #[test]
    fn feeder_windows_and_alignment_targets_are_drawn() {
        let mut d = Document::new();
        let w = feeder_plan(1000.0, 300.0, 10.0).unwrap();
        let g = feeder_windows(&mut d, &w, 300.0, 100.0, 5).unwrap();
        assert_eq!(g.shapes.len(), 8);
        let t = alignment_targets(&mut d, &[Point::new(20.0, 20.0), Point::new(380.0, 20.0)], 8.0, 6).unwrap();
        assert_eq!(t.shapes.len(), 2);
        assert!((t.bounds.x0 - 16.0).abs() < 0.2);
    }
}
