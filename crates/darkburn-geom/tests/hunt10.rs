//! Bug hunt 10: clipping an outline that lies on the clip edge, perforation with input that is
//! not a number, and fitting a grid into an absurdly large area.

use darkburn_geom::array::{Spacing, grid_fit};
use darkburn_geom::clip::clip_stroke;
use darkburn_geom::kurbo::{BezPath, Circle, ParamCurveArclen, Rect, Shape, Vec2};
use darkburn_geom::path::contours;
use darkburn_geom::tabs::perforate;
use std::sync::mpsc;
use std::time::Duration;

fn length(p: &BezPath) -> f64 {
    darkburn_geom::kurbo::segments(p.iter()).map(|s| s.arclen(1e-6)).sum()
}

fn line(x0: f64, x1: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x0, 0.0));
    p.line_to((x1, 0.0));
    p
}

/// Runs `f` on its own thread and gives up after a few seconds: a call that never ends must
/// fail the test, not hang it.
fn within_time<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("{what} did not end"))
}

/// The SVG import case: a 50 x 50 outline clipped by a 20 x 20 rectangle that shares a corner
/// with it. The two sides that run along the clip edge are inside it for 20 mm each, whichever
/// corner it is (the top and right edges used to count as outside).
#[test]
fn an_outline_on_the_clip_edge_is_kept_where_the_edge_is() {
    let outline = Rect::new(0.0, 0.0, 50.0, 50.0).to_path(0.1);
    for (x, y) in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)] {
        let clip = vec![Rect::new(x, y, x + 20.0, y + 20.0).to_path(0.1)];
        let out = clip_stroke(&outline, &clip, 0.01);
        assert!((length(&out) - 40.0).abs() < 1e-6, "clip at {x}, {y}: kept {} mm of the outline, 40 mm lie on the clip edge", length(&out));
        let b = out.bounding_box();
        assert!(b.x0 >= x - 1e-9 && b.y0 >= y - 1e-9 && b.x1 <= x + 20.0 + 1e-9 && b.y1 <= y + 20.0 + 1e-9, "clip at {x}, {y}: {b:?}");
        assert_eq!(contours(&out).len(), 1, "clip at {x}, {y}: the two sides meet at the corner and are one piece");
    }
}

#[test]
fn an_outline_clipped_by_its_own_shape_is_kept_whole() {
    let square = Rect::new(5.0, 5.0, 25.0, 15.0).to_path(0.1);
    let out = clip_stroke(&square, std::slice::from_ref(&square), 0.01);
    assert_eq!(out, square);
    // A curved one too: the clip edge is only known to the flattening tolerance.
    let circle = Circle::new((0.0, 0.0), 10.0).to_path(0.001);
    let out = clip_stroke(&circle, std::slice::from_ref(&circle), 0.01);
    assert!((length(&out) - length(&circle)).abs() < 1e-6, "{} of {}", length(&out), length(&circle));
}

/// What is outside stays outside: a line just past the tolerance from the edge, and one that
/// only comes near the edge at one point.
#[test]
fn a_line_beside_the_clip_edge_is_still_cut_away() {
    let clip = vec![Rect::new(0.0, 0.0, 20.0, 20.0).to_path(0.1)];
    let mut beside = BezPath::new();
    beside.move_to((0.0, -0.5));
    beside.line_to((20.0, -0.5));
    assert!(clip_stroke(&beside, &clip, 0.01).elements().is_empty());
    // Touches the corner with its middle and runs on outside.
    let mut past = BezPath::new();
    past.move_to((10.0, 30.0));
    past.line_to((30.0, 10.0));
    assert!(length(&clip_stroke(&past, &clip, 0.01)) < 0.05);
}

#[test]
fn perforation_with_a_start_that_is_not_a_number_ends() {
    for offset in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e30, -1e30] {
        let dashes = within_time(&format!("perforate with start offset {offset}"), move || perforate(&line(0.0, 100.0), 3.0, 2.0, offset));
        assert!(dashes.elements().is_empty(), "start offset {offset}: there is no telling where the dashes go, so there are none");
    }
    for (cut, skip) in [(f64::INFINITY, 2.0), (3.0, f64::INFINITY), (f64::NAN, 2.0), (3.0, f64::NAN)] {
        let dashes = within_time(&format!("perforate with cut {cut} and skip {skip}"), move || perforate(&line(0.0, 100.0), cut, skip, 0.0));
        assert!(length(&dashes) <= 100.0 + 1e-9, "cut {cut}, skip {skip}: {} mm of dashes on a 100 mm line", length(&dashes));
    }
}

#[test]
fn perforation_of_a_path_with_no_finite_length_ends() {
    let dashes = within_time("perforate on a path of infinite length", || perforate(&line(-1e308, 1e308), 3.0, 2.0, 0.0));
    assert!(dashes.elements().is_empty());
    // An ordinary path next to it is still perforated.
    let mut two = line(-1e308, 1e308);
    two.extend(line(0.0, 100.0).iter());
    let dashes = within_time("perforate beside a path of infinite length", move || perforate(&two, 3.0, 2.0, 0.0));
    assert!((length(&dashes) - 60.0).abs() < 1e-9, "{}", length(&dashes));
}

#[test]
fn fitting_a_grid_into_an_enormous_area_gives_many_copies_not_none() {
    let item = Rect::new(0.0, 0.0, 10.0, 10.0);
    for far in [1e300, f64::INFINITY, 1e22] {
        let (rows, cols) = grid_fit(item, Rect::new(0.0, 0.0, far, far), Spacing::Gap(Vec2::ZERO));
        assert!(rows > 1_000_000 && cols > 1_000_000, "{rows} rows and {cols} columns fit {far} mm");
    }
    assert_eq!(grid_fit(item, Rect::new(0.0, 0.0, 100.0, 35.0), Spacing::Gap(Vec2::new(0.0, 2.0))), (3, 10));
}

/// An offset so far out that its result lies beyond any machine (the Offset command takes any
/// number) gives nothing, as an offset that leaves nothing does; it does not crash.
#[test]
fn an_offset_of_absurd_distance_gives_nothing_and_does_not_crash() {
    use darkburn_geom::offset::{Cap, Join, OffsetOptions, OffsetSide, offset, offset_open};
    let square = Rect::new(0.0, 0.0, 20.0, 10.0).to_path(0.1);
    let open = line(0.0, 30.0);
    for far in [1e300, f64::MAX, f64::INFINITY, 1e13] {
        for side in [OffsetSide::Outward, OffsetSide::Inward, OffsetSide::Both] {
            let o = OffsetOptions { distance: far, side, ..Default::default() };
            let r = within_time("offset", move || offset(&[Rect::new(0.0, 0.0, 20.0, 10.0).to_path(0.1)], &o));
            assert!(r.outward.is_empty() && r.inward.is_empty(), "{far} {side:?}");
        }
        let o = within_time("offset_open", move || offset_open(&line(0.0, 30.0), far, Join::Round, Cap::Round, 0.01));
        assert!(o.is_empty(), "{far}");
    }
    // The largest distance on the farthest shape still taken.
    let o = OffsetOptions { distance: 1e12, side: OffsetSide::Both, ..Default::default() };
    let _ = within_time("offset of a far shape", move || offset(&[Rect::new(-1e12, -1e12, 1e12, 1e12).to_path(0.1)], &o));
    let _ = within_time("offset_open of a far line", || offset_open(&line(-1e12, 1e12), 1e12, Join::Round, Cap::Square, 0.01));
    // Large but within reach still works.
    let o = OffsetOptions { distance: 1e6, ..Default::default() };
    let r = offset(std::slice::from_ref(&square), &o);
    assert_eq!(r.outward.len(), 1);
    assert_eq!(offset_open(&open, 1e6, Join::Round, Cap::Round, 0.01).len(), 1);
}

/// A boolean against hundreds of cutting shapes stacked on one spot (Duplicate, then Align
/// centres) worked through every crossing of every cutter at once: 400 cutters took about 12 s
/// and 830 about two minutes in a test build (seed 7000 of the document monkey), with the app
/// frozen. The cutters are merged first, two by two, so each step stays small.
#[test]
fn a_difference_with_hundreds_of_stacked_cutters_is_quick() {
    use darkburn_geom::FillRule;
    use darkburn_geom::boolean::{difference, union};
    use darkburn_geom::kurbo::{BezPath, Point};
    let polygon = |n: usize, r: f64, turn: f64| {
        let mut p = BezPath::new();
        for i in 0..n {
            let a = turn + i as f64 * std::f64::consts::TAU / n as f64;
            let q = Point::new(100.0 + r * a.cos(), 100.0 + r * a.sin());
            if i == 0 { p.move_to(q) } else { p.line_to(q) }
        }
        p.close_path();
        p
    };
    let subject = vec![polygon(4, 60.0, 0.3)];
    let cutters: Vec<BezPath> = (0..400).map(|i| polygon(24, 20.0, i as f64 * 0.0137)).collect();
    let t = std::time::Instant::now();
    let r = difference(&subject, &cutters, FillRule::EvenOdd, 0.01);
    let took = t.elapsed();
    // The square with a round hole: one piece, its area the square's less the cutters' union.
    let area = |ps: &[BezPath]| ps.iter().map(darkburn_geom::kurbo::Shape::area).sum::<f64>().abs();
    let hole = area(&union(&cutters, FillRule::EvenOdd, 0.01));
    let square = area(&subject);
    assert!((area(&r) - (square - hole)).abs() < 1.0, "area {} expected {}", area(&r), square - hole);
    assert!(took < std::time::Duration::from_secs(3), "400 stacked cutters took {took:?}");
}
