mod common;
use common::*;
use darkburn_text::*;
use kurbo::{BezPath, Circle, Point, Shape};

fn layout(text: &str) -> TextLayout {
    layout_text(&TextSpec { text: text.into(), family: MONO.into(), size: 10.0, ..Default::default() }, &lib()).unwrap()
}

fn line(len: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.line_to((len, 0.0));
    p
}

#[test]
fn straight_path_reproduces_the_flat_layout() {
    let lay = layout("HIJ");
    let out = text_on_path(&lay, &line(200.0), &PathTextOptions::default());
    let flat = lay.bounds;
    let b = out.iter().map(|g| g.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
    assert!((b.x0 - flat.x0).abs() < 1e-6 && (b.x1 - flat.x1).abs() < 1e-6);
    assert!((b.y0 - flat.y0).abs() < 1e-6 && (b.y1 - flat.y1).abs() < 1e-6);
}

#[test]
fn start_offset_and_centering() {
    let lay = layout("HIJ");
    let o = PathTextOptions { start_offset: 25.0, ..Default::default() };
    let b = text_on_path(&lay, &line(200.0), &o).iter().map(|g| g.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
    assert!((b.x0 - (lay.bounds.x0 + 25.0)).abs() < 1e-6);
    let c = PathTextOptions { center_on_path: true, ..Default::default() };
    let b = text_on_path(&lay, &line(200.0), &c).iter().map(|g| g.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
    assert!((b.x0 - (lay.bounds.x0 + 100.0)).abs() < 1e-6);
}

#[test]
fn offset_moves_text_to_the_left_of_the_path() {
    let lay = layout("H");
    let o = PathTextOptions { offset: 7.0, ..Default::default() };
    let b = text_on_path(&lay, &line(50.0), &o)[0].path.bounding_box();
    assert!((b.y0 - (lay.bounds.y0 + 7.0)).abs() < 1e-6);
}

#[test]
fn flip_puts_text_on_the_other_side() {
    let lay = layout("H");
    let o = PathTextOptions { flip: true, ..Default::default() };
    let b = text_on_path(&lay, &line(50.0), &o)[0].path.bounding_box();
    assert!(b.y1 <= 1e-6, "flipped text hangs below the path: {b:?}");
    assert!(b.x0 > 40.0, "and starts at the far end");
}

#[test]
fn rigid_glyphs_keep_their_shape_on_a_circle() {
    let lay = layout("HH");
    let circle = Circle::new((0.0, 0.0), 40.0).to_path(0.01);
    let out = text_on_path(&lay, &circle, &PathTextOptions::default());
    // A rigid glyph keeps its area exactly.
    for (g, o) in lay.glyphs.iter().zip(out.iter()) {
        assert!((g.path.area().abs() - o.path.area().abs()).abs() < 1e-6);
    }
    // The second H is rotated relative to the first.
    let c0 = out[0].path.bounding_box().center();
    let c1 = out[1].path.bounding_box().center();
    assert!((c0 - c1).hypot() > 5.0);
}

#[test]
fn distort_bends_outlines_around_a_circle() {
    let lay = layout("HELLO");
    let r = 60.0;
    let circle = Circle::new((0.0, 0.0), r).to_path(0.01);
    let out = text_on_path(&lay, &circle, &PathTextOptions { distort: true, ..Default::default() });
    let cap = lay.metrics.cap_height_mm;
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for g in &out {
        for el in g.path.elements() {
            if let kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) = el {
                let d = (*p - Point::ORIGIN).hypot();
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    assert!(hi - lo > cap * 0.9 && hi - lo < cap + 0.5, "radial spread {} for cap {}", hi - lo, cap);
    assert!(lo > r - cap - 0.5 && hi < r + cap + 0.5);
}

#[test]
fn closed_paths_wrap_and_open_paths_extrapolate() {
    let lay = layout("HELLO WORLD");
    // Text longer than a short open path still gets placed (continues along the end tangent).
    let out = text_on_path(&lay, &line(10.0), &PathTextOptions::default());
    assert_eq!(out.len(), lay.glyphs.len());
    let far = out.last().unwrap().path.bounding_box();
    assert!(far.x1 > 10.0);
    // Closed square path: sampler wraps.
    let mut sq = BezPath::new();
    sq.move_to((0.0, 0.0));
    sq.line_to((10.0, 0.0));
    sq.line_to((10.0, 10.0));
    sq.line_to((0.0, 10.0));
    sq.close_path();
    let s = PathSampler::new(&sq, 0.01).unwrap();
    assert!((s.length() - 40.0).abs() < 1e-9 && s.is_closed());
    let (p, _) = s.at(45.0);
    assert!((p - Point::new(5.0, 0.0)).hypot() < 1e-9);
}

#[test]
fn apply_path_to_text_returns_bent_outlines() {
    let lay = layout("ARC");
    let arc = kurbo::Arc::new((0.0, 0.0), (50.0, 50.0), 0.0, std::f64::consts::PI, 0.0).to_path(0.01);
    let out = apply_path_to_text(&lay, &arc);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|p| p.bounding_box().max_y() > 5.0));
    // Empty path gives nothing instead of panicking.
    assert!(apply_path_to_text(&lay, &BezPath::new()).is_empty());
}
