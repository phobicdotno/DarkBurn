use super::*;
use crate::path::{contours, flatten_contour};
use kurbo::{Circle, Shape};
use std::time::Instant;

fn rect(x: f64, y: f64, w: f64, h: f64) -> BezPath {
    Rect::new(x, y, x + w, y + h).to_path(0.1)
}

fn poly(pts: &[(f64, f64)]) -> BezPath {
    let mut p = BezPath::new();
    p.move_to(pts[0]);
    for q in &pts[1..] {
        p.line_to(*q);
    }
    p.close_path();
    p
}

fn circle(x: f64, y: f64, r: f64) -> BezPath {
    Circle::new((x, y), r).to_path(0.01)
}

fn star(x: f64, y: f64, r: f64) -> BezPath {
    let pts: Vec<(f64, f64)> = (0..10)
        .map(|i| {
            let a = i as f64 * std::f64::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (x + rr * a.cos(), y + rr * a.sin())
        })
        .collect();
    poly(&pts)
}

fn part(p: BezPath) -> NestPart {
    NestPart::cut(vec![p])
}

fn all_true(_: NestProgress) -> bool {
    true
}

fn quick(parts: &[NestPart], obstacles: &[NestPart], sheet: Rect, o: &SheetNestOptions) -> SheetNestResult {
    quick_nest(parts, obstacles, sheet, o, &mut all_true).unwrap()
}

fn shaped(parts: &[NestPart], obstacles: &[NestPart], sheet: Rect, o: &SheetNestOptions) -> SheetNestResult {
    true_nest(parts, obstacles, sheet, o, &mut all_true).unwrap()
}

/// A placed part flattened finely: contours as (points, closed).
fn world(p: &NestPart, xf: Affine) -> Vec<(Vec<Point>, bool)> {
    p.all()
        .flat_map(|path| contours(&(xf * path.clone())))
        .map(|c| {
            let pl = flatten_contour(&c, 0.005);
            let closed = c.is_closed_within(1e-9);
            (pl.points, closed)
        })
        .filter(|c| !c.0.is_empty())
        .collect()
}

fn edges(c: &(Vec<Point>, bool)) -> Vec<(Point, Point)> {
    let n = c.0.len();
    if n == 1 {
        return vec![(c.0[0], c.0[0])];
    }
    let m = if c.1 { n } else { n - 1 };
    (0..m).map(|i| (c.0[i], c.0[(i + 1) % n])).collect()
}

fn inside_even_odd(cs: &[(Vec<Point>, bool)], p: Point) -> bool {
    cs.iter().filter(|c| c.1 && c.0.len() >= 3 && space::in_ring(&c.0, p)).count() % 2 == 1
}

fn bounds(cs: &[(Vec<Point>, bool)]) -> Rect {
    let mut r = Rect::from_points(cs[0].0[0], cs[0].0[0]);
    for p in cs.iter().flat_map(|c| &c.0) {
        r = r.union_pt(*p);
    }
    r
}

/// Independent check of a layout on the real (finely flattened) shapes: inside the sheet margin,
/// the gap kept between every pair and to obstacles, nothing inside anything else.
fn check_layout(parts: &[NestPart], obstacles: &[NestPart], sheet: Rect, o: &SheetNestOptions, r: &SheetNestResult) {
    let slack = 0.025;
    let mut placed: Vec<(usize, Vec<(Vec<Point>, bool)>)> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        let Some(xf) = r.placements[i] else { continue };
        let k = r.sheets[i].unwrap();
        let usable = sheet_rect(sheet, o.sheet_spacing, k).inset(-o.margin);
        let w = world(p, xf);
        if w.is_empty() {
            continue;
        }
        let b = bounds(&w);
        assert!(
            b.x0 >= usable.x0 - slack && b.y0 >= usable.y0 - slack && b.x1 <= usable.x1 + slack && b.y1 <= usable.y1 + slack,
            "part {i} at {b:?} is outside {usable:?}"
        );
        placed.push((k, w));
    }
    for ob in obstacles {
        let w = world(ob, Affine::IDENTITY);
        if !w.is_empty() {
            placed.push((usize::MAX, w));
        }
    }
    let n_parts = placed.len() - obstacles.len();
    for i in 0..n_parts {
        for j in i + 1..placed.len() {
            let (ka, a) = &placed[i];
            let (kb, b) = &placed[j];
            let same = ka == kb || (*kb == usize::MAX && *ka == 0);
            if !same {
                continue;
            }
            let (ba, bb) = (bounds(a), bounds(b));
            if !ba.inflate(o.gap, o.gap).overlaps(bb) {
                continue;
            }
            for ea in a.iter().flat_map(edges) {
                for eb in b.iter().flat_map(edges) {
                    let d = space::seg_dist(ea.0, ea.1, eb.0, eb.1);
                    assert!(d >= o.gap - slack, "parts {i} and {j} are {d} apart, gap {}", o.gap);
                }
            }
            for c in a {
                assert!(!inside_even_odd(b, c.0[0]), "part {i} lies inside {j}");
            }
            for c in b {
                assert!(!inside_even_odd(a, c.0[0]), "part {j} lies inside {i}");
            }
        }
    }
}

fn placed_count(r: &SheetNestResult) -> usize {
    r.placements.iter().filter(|p| p.is_some()).count()
}

fn opts(gap: f64) -> SheetNestOptions {
    SheetNestOptions { gap, ..Default::default() }
}

#[test]
fn quick_packs_rectangles_inside_with_gap() {
    let parts: Vec<NestPart> =
        (0..30).map(|i| part(rect(500.0 + i as f64 * 7.0, -40.0, 5.0 + (i % 7) as f64 * 3.0, 4.0 + (i % 5) as f64 * 2.0))).collect();
    let sheet = Rect::new(10.0, 10.0, 210.0, 160.0);
    let o = SheetNestOptions { gap: 2.0, margin: 3.0, ..Default::default() };
    let r = quick(&parts, &[], sheet, &o);
    assert_eq!(r.unplaced, 0);
    assert_eq!(r.sheets_used, 1);
    assert!(r.utilization > 0.0 && r.utilization <= 1.0);
    assert!(!r.stopped);
    check_layout(&parts, &[], sheet, &o, &r);
}

#[test]
fn quick_turns_parts_to_fit() {
    let parts = vec![part(rect(0.0, 0.0, 80.0, 20.0))];
    let sheet = Rect::new(0.0, 0.0, 30.0, 100.0);
    let r = quick(&parts, &[], sheet, &opts(1.0));
    assert_eq!(r.unplaced, 0);
    check_layout(&parts, &[], sheet, &opts(1.0), &r);
    let fixed = SheetNestOptions { rotations: quarter_turns(false), ..opts(1.0) };
    assert_eq!(quick(&parts, &[], sheet, &fixed).unplaced, 1);
    assert_eq!(shaped(&parts, &[], sheet, &fixed).unplaced, 1);
    assert_eq!(shaped(&parts, &[], sheet, &opts(1.0)).unplaced, 0);
}

#[test]
fn both_spill_onto_more_sheets_side_by_side() {
    let parts: Vec<NestPart> = (0..6).map(|_| part(rect(0.0, 0.0, 40.0, 40.0))).collect();
    let sheet = Rect::new(0.0, 0.0, 100.0, 50.0);
    for nest in [quick_nest, true_nest] {
        let o = SheetNestOptions { max_sheets: 5, sheet_spacing: 20.0, ..opts(2.0) };
        let r = nest(&parts, &[], sheet, &o, &mut all_true).unwrap();
        assert_eq!(r.unplaced, 0);
        assert_eq!(r.sheets_used, 3);
        check_layout(&parts, &[], sheet, &o, &r);
        let one = nest(&parts, &[], sheet, &opts(2.0), &mut all_true).unwrap();
        assert_eq!(one.unplaced, 4);
        assert_eq!(one.sheets_used, 1);
    }
    assert_eq!(sheet_rect(sheet, 20.0, 2), Rect::new(240.0, 0.0, 340.0, 50.0));
}

#[test]
fn both_keep_clear_of_obstacles() {
    let parts: Vec<NestPart> = (0..12).map(|i| part(rect(0.0, 0.0, 15.0 + i as f64, 12.0))).collect();
    let obstacles = vec![part(circle(60.0, 40.0, 25.0)), part(rect(0.0, 0.0, 20.0, 20.0))];
    let sheet = Rect::new(0.0, 0.0, 140.0, 90.0);
    for nest in [quick_nest, true_nest] {
        let r = nest(&parts, &obstacles, sheet, &opts(3.0), &mut all_true).unwrap();
        assert_eq!(r.unplaced, 0);
        check_layout(&parts, &obstacles, sheet, &opts(3.0), &r);
    }
}

#[test]
fn a_frame_around_the_sheet_counts_by_its_line() {
    let parts: Vec<NestPart> = (0..4).map(|_| part(rect(0.0, 0.0, 20.0, 20.0))).collect();
    let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
    let frame = vec![part(rect(-1.0, -1.0, 102.0, 102.0))];
    for nest in [quick_nest, true_nest] {
        let r = nest(&parts, &frame, sheet, &opts(2.0), &mut all_true).unwrap();
        assert_eq!(r.unplaced, 0);
        check_layout(&parts, &frame, sheet, &opts(2.0), &r);
    }
}

fn mixed() -> Vec<NestPart> {
    let mut parts = Vec::new();
    for i in 0..6 {
        parts.push(part(circle(0.0, 0.0, 6.0 + i as f64)));
        parts.push(part(star(100.0, 50.0, 12.0 + i as f64)));
        parts.push(part(poly(&[(0.0, 0.0), (30.0, 0.0), (30.0, 8.0), (8.0, 8.0), (8.0, 25.0), (0.0, 25.0)])));
        parts.push(part(poly(&[(0.0, 0.0), (25.0 + i as f64, 0.0), (0.0, 20.0)])));
    }
    // An open line and a part with an engraving inside its cut outline.
    let mut line = BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((30.0, 12.0));
    parts.push(NestPart { cuts: vec![], marks: vec![line] });
    parts.push(NestPart { cuts: vec![rect(0.0, 0.0, 30.0, 20.0)], marks: vec![circle(15.0, 10.0, 5.0)] });
    parts
}

#[test]
fn true_nest_mixed_shapes_keep_the_gap() {
    let parts = mixed();
    let sheet = Rect::new(-50.0, 20.0, 250.0, 220.0);
    let o = SheetNestOptions { gap: 3.0, margin: 2.0, ..Default::default() };
    let r = shaped(&parts, &[], sheet, &o);
    assert_eq!(r.unplaced, 0);
    assert!(!r.stopped);
    check_layout(&parts, &[], sheet, &o, &r);
}

#[test]
fn true_nest_interlocks_where_boxes_cannot() {
    let parts: Vec<NestPart> = (0..10).map(|_| part(poly(&[(0.0, 0.0), (40.0, 0.0), (0.0, 40.0)]))).collect();
    let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
    let q = quick(&parts, &[], sheet, &opts(2.0));
    let t = shaped(&parts, &[], sheet, &opts(2.0));
    check_layout(&parts, &[], sheet, &opts(2.0), &q);
    check_layout(&parts, &[], sheet, &opts(2.0), &t);
    assert_eq!(placed_count(&q), 4);
    assert!(placed_count(&t) >= 6, "true nest placed {}", placed_count(&t));
}

fn frame_and_squares() -> Vec<NestPart> {
    let mut parts = vec![NestPart::cut(vec![rect(0.0, 0.0, 100.0, 100.0), rect(10.0, 10.0, 80.0, 80.0)])];
    parts.extend((0..4).map(|_| part(rect(0.0, 0.0, 30.0, 30.0))));
    parts
}

#[test]
fn true_nest_puts_small_parts_in_holes_when_allowed() {
    let parts = frame_and_squares();
    let sheet = Rect::new(0.0, 0.0, 104.0, 104.0);
    let o = opts(2.0);
    let r = shaped(&parts, &[], sheet, &o);
    assert_eq!(r.unplaced, 0);
    check_layout(&parts, &[], sheet, &o, &r);
    let off = SheetNestOptions { allow_holes: false, ..opts(2.0) };
    let r = shaped(&parts, &[], sheet, &off);
    assert!(r.placements[0].is_some());
    assert_eq!(r.unplaced, 4);
}

#[test]
fn obstacle_holes_take_parts_too() {
    let obstacles = vec![NestPart::cut(vec![rect(0.0, 0.0, 100.0, 100.0), rect(20.0, 20.0, 60.0, 60.0)])];
    let parts = vec![part(rect(0.0, 0.0, 40.0, 40.0))];
    let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
    let r = shaped(&parts, &obstacles, sheet, &opts(2.0));
    assert_eq!(r.unplaced, 0);
    check_layout(&parts, &obstacles, sheet, &opts(2.0), &r);
}

#[test]
fn true_nest_is_repeatable() {
    let parts = mixed();
    let sheet = Rect::new(0.0, 0.0, 300.0, 200.0);
    let a = shaped(&parts, &[], sheet, &opts(2.0));
    let b = shaped(&parts, &[], sheet, &opts(2.0));
    assert_eq!(a, b);
    let c = quick(&parts, &[], sheet, &opts(2.0));
    let d = quick(&parts, &[], sheet, &opts(2.0));
    assert_eq!(c, d);
}

#[test]
fn true_nest_beats_or_matches_quick_on_mixed_parts() {
    let parts = mixed();
    let sheet = Rect::new(0.0, 0.0, 160.0, 120.0);
    let q = quick(&parts, &[], sheet, &opts(2.0));
    let t = shaped(&parts, &[], sheet, &opts(2.0));
    check_layout(&parts, &[], sheet, &opts(2.0), &q);
    check_layout(&parts, &[], sheet, &opts(2.0), &t);
    assert!(placed_count(&t) >= placed_count(&q), "true {} quick {}", placed_count(&t), placed_count(&q));
}

#[test]
fn cancel_keeps_the_best_result_so_far() {
    let parts: Vec<NestPart> = (0..30).map(|i| part(star(0.0, 0.0, 8.0 + (i % 5) as f64))).collect();
    let sheet = Rect::new(0.0, 0.0, 200.0, 200.0);
    let mut seen = Vec::new();
    let mut stop_after_five = |p: NestProgress| {
        seen.push(p);
        p.done < 5
    };
    let r = true_nest(&parts, &[], sheet, &opts(2.0), &mut stop_after_five).unwrap();
    assert!(r.stopped);
    let n = placed_count(&r);
    assert!((1..30).contains(&n), "{n} placed");
    check_layout(&parts, &[], sheet, &opts(2.0), &r);
    assert!(seen.iter().all(|p| p.total >= 30 && p.done <= p.total));
    let mut stop_now = |_: NestProgress| false;
    let r = quick_nest(&parts, &[], sheet, &opts(2.0), &mut stop_now).unwrap();
    assert!(r.stopped);
    assert_eq!(r.unplaced, 30);
}

#[test]
fn time_limit_bounds_a_big_run() {
    let parts: Vec<NestPart> = (0..3000).map(|i| part(circle(0.0, 0.0, 2.0 + (i % 4) as f64))).collect();
    let sheet = Rect::new(0.0, 0.0, 300.0, 200.0);
    let o = SheetNestOptions { time_limit: Duration::from_millis(300), ..opts(1.0) };
    let t0 = Instant::now();
    let r = shaped(&parts, &[], sheet, &o);
    assert!(t0.elapsed() < Duration::from_secs(20), "took {:?}", t0.elapsed());
    assert!(r.stopped);
    assert_eq!(r.placements.len(), 3000);
    let placed: Vec<NestPart> = parts.iter().zip(&r.placements).filter(|(_, p)| p.is_some()).map(|(p, _)| p.clone()).collect();
    assert!(!placed.is_empty());
    let sub = SheetNestResult {
        placements: r.placements.iter().filter(|p| p.is_some()).copied().collect(),
        sheets: r.sheets.iter().filter(|p| p.is_some()).copied().collect(),
        ..r.clone()
    };
    check_layout(&placed, &[], sheet, &o, &sub);
}

#[test]
fn thousands_of_boxes_nest_quickly() {
    let parts: Vec<NestPart> = (0..3000).map(|i| part(rect(0.0, 0.0, 4.0 + (i % 3) as f64, 5.0))).collect();
    let sheet = Rect::new(0.0, 0.0, 600.0, 400.0);
    let t0 = Instant::now();
    let r = quick(&parts, &[], sheet, &SheetNestOptions { time_limit: Duration::from_secs(60), ..opts(1.0) });
    assert!(t0.elapsed() < Duration::from_secs(30), "took {:?}", t0.elapsed());
    assert_eq!(r.unplaced, 0);
    let mut boxes: Vec<Rect> = parts.iter().zip(&r.placements).map(|(p, xf)| xf.unwrap().transform_rect_bbox(part_bounds(p).unwrap())).collect();
    boxes.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    for i in 0..boxes.len() {
        for j in i + 1..boxes.len() {
            if boxes[j].x0 > boxes[i].x1 + 1.0 {
                break;
            }
            let gx = (boxes[j].x0 - boxes[i].x1).max(boxes[i].x0 - boxes[j].x1);
            let gy = (boxes[j].y0 - boxes[i].y1).max(boxes[i].y0 - boxes[j].y1);
            assert!(gx.max(gy) >= 1.0 - 1e-6);
        }
    }
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
    let mut nan = BezPath::new();
    nan.move_to((0.0, 0.0));
    nan.line_to((f64::NAN, 3.0));
    let far = rect(1e300, 0.0, 1.0, 1.0);
    let ok = vec![part(rect(0.0, 0.0, 5.0, 5.0))];
    for nest in [quick_nest, true_nest] {
        let o = opts(1.0);
        assert_eq!(nest(&[part(nan.clone())], &[], sheet, &o, &mut all_true), Err(SheetNestError::NotFinite));
        assert_eq!(nest(&ok, &[part(far.clone())], sheet, &o, &mut all_true), Err(SheetNestError::NotFinite));
        assert_eq!(nest(&ok, &[], Rect::new(0.0, 0.0, 0.0, 10.0), &o, &mut all_true), Err(SheetNestError::Sheet));
        assert_eq!(nest(&ok, &[], Rect::new(0.0, 0.0, f64::NAN, 10.0), &o, &mut all_true), Err(SheetNestError::Sheet));
        let wide = SheetNestOptions { margin: 50.0, ..opts(1.0) };
        assert_eq!(nest(&ok, &[], sheet, &wide, &mut all_true), Err(SheetNestError::Sheet));
        let neg = SheetNestOptions { gap: -1.0, ..opts(1.0) };
        assert_eq!(nest(&ok, &[], sheet, &neg, &mut all_true), Err(SheetNestError::Option("gap")));
        let rot = SheetNestOptions { rotations: vec![f64::INFINITY], ..opts(1.0) };
        assert_eq!(nest(&ok, &[], sheet, &rot, &mut all_true), Err(SheetNestError::Option("rotation")));
        let tol = SheetNestOptions { tolerance: 0.0, ..opts(1.0) };
        assert_eq!(nest(&ok, &[], sheet, &tol, &mut all_true), Err(SheetNestError::Option("tolerance")));
        let many = vec![NestPart::default(); MAX_PARTS + 1];
        assert_eq!(nest(&many, &[], sheet, &o, &mut all_true), Err(SheetNestError::TooManyParts(MAX_PARTS + 1)));
    }
    assert!(!SheetNestError::Sheet.to_string().is_empty());
}

#[test]
fn degenerate_parts_are_skipped_or_placed_safely() {
    let mut dot = BezPath::new();
    dot.move_to((5.0, 5.0));
    let mut flat = BezPath::new();
    flat.move_to((0.0, 0.0));
    flat.line_to((20.0, 0.0));
    flat.line_to((10.0, 0.0));
    flat.close_path();
    let mut line = BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((0.0, 30.0));
    let parts = vec![NestPart::default(), part(dot), part(flat), part(line.clone()), part(line), part(rect(0.0, 0.0, 10.0, 10.0))];
    let sheet = Rect::new(0.0, 0.0, 60.0, 60.0);
    for nest in [quick_nest, true_nest] {
        let r = nest(&parts, &[], sheet, &opts(2.0), &mut all_true).unwrap();
        assert!(r.placements[0].is_none());
        assert!(r.placements[1].is_none());
        assert!(r.placements[2..].iter().all(Option::is_some), "{:?}", r.placements);
        assert_eq!(r.unplaced, 2);
        check_layout(&parts, &[], sheet, &opts(2.0), &r);
    }
}

#[test]
fn huge_outlines_are_simplified_and_still_kept_apart() {
    // A wavy ring of 60 000 points, plus small squares around it.
    let n = 60_000;
    let pts: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let r = 40.0 + 2.0 * (a * 40.0).sin();
            (r * a.cos(), r * a.sin())
        })
        .collect();
    let mut parts = vec![part(poly(&pts))];
    parts.extend((0..8).map(|_| part(rect(0.0, 0.0, 12.0, 12.0))));
    let sheet = Rect::new(0.0, 0.0, 130.0, 100.0);
    let t0 = Instant::now();
    let r = shaped(&parts, &[], sheet, &opts(2.0));
    assert!(t0.elapsed() < Duration::from_secs(20), "took {:?}", t0.elapsed());
    assert_eq!(r.unplaced, 0);
    check_layout(&parts, &[], sheet, &opts(2.0), &r);
}

#[test]
fn fine_rotation_steps_are_thinned() {
    let o = SheetNestOptions { rotations: crate::nest::rotation_steps(0.1), ..opts(2.0) };
    let parts: Vec<NestPart> = (0..3).map(|_| part(poly(&[(0.0, 0.0), (30.0, 0.0), (0.0, 12.0)]))).collect();
    let sheet = Rect::new(0.0, 0.0, 80.0, 60.0);
    let r = shaped(&parts, &[], sheet, &o);
    assert_eq!(r.unplaced, 0);
    check_layout(&parts, &[], sheet, &o, &r);
    assert_eq!(rotations(&o.rotations).unwrap().len(), MAX_ROTATIONS);
    assert_eq!(rotations(&[0.0, 360.0, -90.0, 270.0]).unwrap(), vec![0.0, 270.0]);
}

#[test]
fn capsule_and_distance_helpers() {
    use kurbo::Vec2;
    let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
    // Falling onto a segment from above stops at the clearance.
    let t = space::hit_capsule(Point::new(5.0, 10.0), Vec2::new(0.0, -1.0), a, b, 2.0).unwrap();
    assert!((t - 8.0).abs() < 1e-9);
    // Sliding along at exactly the clearance is free; moving away never hits.
    assert_eq!(space::hit_capsule(Point::new(-5.0, 2.0), Vec2::new(1.0, 0.0), a, b, 2.0 - 1e-6), None);
    assert_eq!(space::hit_capsule(Point::new(5.0, 3.0), Vec2::new(0.0, 1.0), a, b, 2.0), None);
    // Past the end, the round cap is hit.
    let t = space::hit_capsule(Point::new(12.0, 10.0), Vec2::new(0.0, -1.0), a, b, 2.0).unwrap();
    assert!((t - 10.0).abs() < 1e-9);
    assert_eq!(space::seg_dist(a, b, Point::new(5.0, -1.0), Point::new(5.0, 1.0)), 0.0);
    assert!((space::seg_dist(a, b, Point::new(12.0, 0.0), Point::new(15.0, 0.0)) - 2.0).abs() < 1e-12);
}
