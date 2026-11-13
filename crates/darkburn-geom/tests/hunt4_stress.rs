//! Degenerate and extreme input to the shape operations: nothing may panic or run on for ever.
//! Every (operation, input) pair runs on its own thread with a time limit, and all failures of a
//! run are listed together.

use darkburn_geom::kurbo::{BezPath, Circle, Point, Rect, Shape, Vec2};
use darkburn_geom::node::NodeRef;
use darkburn_geom::offset::{Cap, Join, OffsetOptions, OffsetSide};
use std::sync::mpsc;
use std::time::Duration;

fn svg(s: &str) -> BezPath {
    BezPath::from_svg(s).unwrap()
}

fn poly(pts: &[(f64, f64)], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            p.move_to((x, y));
        } else {
            p.line_to((x, y));
        }
    }
    if closed {
        p.close_path();
    }
    p
}

/// The nasty inputs, by name.
pub fn inputs() -> Vec<(&'static str, BezPath)> {
    let mut v: Vec<(&'static str, BezPath)> = vec![
        ("empty", BezPath::new()),
        ("lone move", svg("M5 5")),
        ("move close", svg("M5 5 Z")),
        ("zero line", svg("M5 5 L5 5")),
        ("zero line closed", svg("M5 5 L5 5 Z")),
        ("zero triangle", svg("M5 5 L5 5 L5 5 Z")),
        ("collinear closed", svg("M0 0 L10 0 L20 0 Z")),
        ("out and back", svg("M0 0 L10 0 L0 0 Z")),
        ("bow tie", svg("M0 0 L10 10 L10 0 L0 10 Z")),
        ("figure eight at a node", svg("M0 0 L10 0 L10 10 L0 10 Z M10 10 L20 10 L20 20 L10 20 Z")),
        ("self touching", svg("M0 0 L20 0 L20 10 L10 10 L10 20 L20 20 L20 10 L30 10 L30 30 L0 30 Z")),
        ("duplicate nodes", svg("M0 0 L0 0 L10 0 L10 0 L10 10 L10 10 L0 10 L0 10 Z")),
        ("open polyline", svg("M0 0 L10 0 L10 10")),
        ("open one line", svg("M0 0 L10 0")),
        ("open ends meet", svg("M0 0 L10 0 L10 10 L0 0")),
        ("point cubic", svg("M3 3 C3 3 3 3 3 3 Z")),
        ("flat cubic", svg("M0 0 C5 0 10 0 15 0")),
        ("cusp cubic", svg("M0 0 C10 10 0 10 10 0 Z")),
        ("loop cubic", svg("M0 0 C30 20 -20 20 10 0")),
        ("line after close", svg("M0 0 L10 0 L10 10 Z L5 5 L7 9")),
        ("tiny circle", Circle::new((5.0, 5.0), 1e-9).to_path(1e-12)),
        ("tiny square", Rect::new(0.0, 0.0, 1e-7, 1e-7).to_path(0.1)),
        ("sliver", svg("M0 0 L100 0 L100 1e-9 Z")),
        ("huge square", Rect::new(-1e9, -1e9, 1e9, 1e9).to_path(0.1)),
        ("far square", Rect::new(1e11, 1e11, 1e11 + 10.0, 1e11 + 10.0).to_path(0.1)),
        ("absurd square", Rect::new(-1e300, -1e300, 1e300, 1e300).to_path(0.1)),
        ("max square", Rect::new(-f64::MAX, -f64::MAX, f64::MAX, f64::MAX).to_path(0.1)),
        ("overflowing triangle", svg("M-1e308 -1e308 L1e308 1e308 L1e308 -1e308 Z")),
        ("overflowing cubic", svg("M0 0 C1e308 1e308 -1e308 1e308 0 5 Z")),
        ("overflowing open cubic", svg("M0 0 C1e308 1e308 -1e308 1e308 0 5 L5 5")),
        ("nan node", poly(&[(0.0, 0.0), (10.0, 0.0), (f64::NAN, 10.0), (0.0, 10.0)], true)),
        ("nan open", poly(&[(0.0, 0.0), (f64::NAN, f64::NAN), (10.0, 10.0)], false)),
        ("all nan", poly(&[(f64::NAN, f64::NAN), (f64::NAN, f64::NAN), (f64::NAN, f64::NAN)], true)),
        ("inf node", poly(&[(0.0, 0.0), (10.0, 0.0), (f64::INFINITY, 10.0), (0.0, 10.0)], true)),
        ("neg inf open", poly(&[(0.0, 0.0), (f64::NEG_INFINITY, 0.0)], false)),
        ("spiky", svg("M0 0 L10 0 L10 10 L5 10 L5 1000000 L5 10 L0 10 Z")),
        ("square with hole same winding", svg("M0 0 L20 0 L20 20 L0 20 Z M5 5 L15 5 L15 15 L5 15 Z")),
        ("two same squares", svg("M0 0 L10 0 L10 10 L0 10 Z M0 0 L10 0 L10 10 L0 10 Z")),
        ("mixed open closed", svg("M0 0 L10 0 L10 10 Z M20 0 L30 0 L30 10")),
        ("quad", svg("M0 0 Q5 10 10 0 Z")),
        ("subnormal", poly(&[(0.0, 0.0), (5e-324, 0.0), (5e-324, 5e-324)], true)),
    ];
    // A closed outline with very many nodes on one spot and a real square round them.
    let mut many = vec![(0.0, 0.0); 2000];
    many.extend([(10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    v.push(("2000 nodes on one spot", poly(&many, true)));
    v
}

type Op = (&'static str, fn(&BezPath));

fn noop_progress(_: usize, _: usize) -> bool {
    true
}

fn ops() -> Vec<Op> {
    use darkburn_geom::*;
    let v: Vec<Op> = vec![
        ("union", |p| {
            for r in [FillRule::EvenOdd, FillRule::NonZero] {
                let _ = boolean::union(&[p.clone(), Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1)], r, 0.01);
                let _ = boolean::union(std::slice::from_ref(p), r, 0.01);
            }
        }),
        ("difference", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = boolean::difference(std::slice::from_ref(p), std::slice::from_ref(&sq), FillRule::EvenOdd, 0.01);
            let _ = boolean::difference(&[sq], std::slice::from_ref(p), FillRule::EvenOdd, 0.01);
        }),
        ("intersection", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = boolean::intersection(std::slice::from_ref(p), &[sq], FillRule::NonZero, 0.01);
        }),
        ("xor", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = boolean::xor(std::slice::from_ref(p), &[sq], FillRule::EvenOdd, 0.01);
        }),
        ("weld", |p| {
            let _ = boolean::weld(&[p.clone(), p.clone()], FillRule::EvenOdd, 0.01);
        }),
        ("cut_shapes", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = boolean::cut_shapes(&[p.clone(), sq, p.clone()], FillRule::EvenOdd, 0.01);
        }),
        ("resolve", |p| {
            let _ = boolean::resolve(std::slice::from_ref(p), FillRule::EvenOdd, 0.01);
        }),
        ("offset", |p| {
            for join in [Join::Round, Join::Bevel, Join::Miter { limit: 4.0 }] {
                for d in [0.5, 1e-9, 1e6] {
                    for outer_only in [false, true] {
                        let o = OffsetOptions { distance: d, side: OffsetSide::Both, join, cap: Cap::Round, outer_only, ..Default::default() };
                        let _ = offset::offset(std::slice::from_ref(p), &o);
                    }
                }
            }
        }),
        ("offset_open", |p| {
            for cap in [Cap::Round, Cap::Butt, Cap::Square] {
                let _ = offset::offset_open(p, 1.0, Join::Round, cap, 0.01);
            }
        }),
        ("kerf_offset", |p| {
            for k in [0.1, -0.1, 50.0, -50.0, 1e-12] {
                let _ = offset::kerf_offset_counted(p, k, 0.01);
            }
        }),
        ("close_path", |p| {
            let _ = cleanup::close_path(p);
            let _ = cleanup::close_paths_with_tolerance(p, 0.1);
            let _ = cleanup::close_paths_with_tolerance(p, f64::INFINITY);
        }),
        ("auto_join", |p| {
            for tol in [0.0, 0.05, 1e-12, 1e9] {
                let _ = cleanup::auto_join(&[p.clone(), p.clone(), svg("M10 10 L20 20")], tol, &mut noop_progress);
                let _ = cleanup::auto_join(&path::subpaths(p), tol, &mut noop_progress);
            }
        }),
        ("delete_duplicates", |p| {
            let _ = cleanup::delete_duplicates(&[p.clone(), p.clone()], 0.01);
        }),
        ("delete_overlapping_lines", |p| {
            let _ = cleanup::delete_overlapping_lines(&[p.clone(), p.clone()], 0.01);
        }),
        ("break_apart", |p| {
            let _ = cleanup::break_apart(p, true, 0.01);
            let _ = cleanup::break_apart(p, false, 0.01);
        }),
        ("optimize", |p| {
            for tol in [0.0, 0.01, 10.0, 1e9] {
                let _ = cleanup::optimize(p, tol);
            }
        }),
        ("flatten", |p| {
            let _ = cleanup::flatten_curves(p, 0.01);
            let _ = path::flatten(p, 0.0);
        }),
        ("round_corners", |p| {
            for r in [0.0, 1.0, 1e9, -1.0] {
                let _ = fillet::round_corners(p, r, 0.01);
                let _ = fillet::chamfer(p, r, 0.01);
            }
            let _ = fillet::round_corners_at(p, &[NodeRef::new(0, 0), NodeRef::new(0, 99), NodeRef::new(7, 0)], 1.0, 0.01);
        }),
        ("unfillet", |p| {
            let _ = unfillet::remove_rounded_corners(p, 0.01);
            let _ = unfillet::count_rounded_corners(p, 0.01);
        }),
        ("node edits", |p| {
            for sub in 0..3 {
                for i in 0..6 {
                    let n = NodeRef::new(sub, i);
                    let _ = node::insert_node(p, sub, i, 0.5);
                    let _ = node::insert_node(p, sub, i, 0.0);
                    let _ = node::insert_node(p, sub, i, f64::NAN);
                    let _ = node::delete_node(p, n);
                    let _ = node::delete_segment(p, sub, i);
                    let _ = node::break_at_node(p, n);
                    let _ = node::segment_to_line(p, sub, i);
                    let _ = node::segment_to_curve(p, sub, i);
                    let _ = node::set_node_corner(p, n);
                    let _ = node::set_node_smooth(p, n);
                    let _ = node::move_node(p, n, Vec2::new(1.0, 1.0));
                    let _ = node::extend_to_intersection(p, &[Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1)], n, 100.0);
                }
            }
            let all: Vec<NodeRef> = (0..6).map(|i| NodeRef::new(0, i)).collect();
            for a in [node::Align::Left, node::Align::Top, node::Align::CenterH, node::Align::CenterV] {
                let _ = node::align_nodes(p, &all, a);
            }
            let _ = node::trim_at_intersection(p, &[Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1)], Point::new(5.0, 0.0));
        }),
        ("node delete to nothing", |p| {
            // Delete node 0 of subpath 0 until nothing is left or it is refused.
            let mut q = p.clone();
            for _ in 0..5000 {
                match node::delete_node(&q, NodeRef::new(0, 0)) {
                    Ok(n) => q = n,
                    Err(_) => break,
                }
            }
        }),
        ("tabs", |p| {
            let _ = tabs::place_tabs(p, &tabs::TabOptions::default());
            let _ = tabs::place_tabs(p, &tabs::TabOptions { count: 1000, size: 1e-3, ..Default::default() });
            let _ = tabs::place_tabs(p, &tabs::TabOptions { spacing: Some(0.0), ..Default::default() });
            let _ = tabs::place_tabs(p, &tabs::TabOptions { spacing: Some(f64::NAN), size: f64::NAN, ..Default::default() });
            let _ = tabs::place_tabs_at(p, &[0.0, -5.0, 1e9, f64::NAN], 1.0);
            // `tabs::perforate` is left out: nothing in the app calls it (see BUGHUNT-NOTES).
        }),
        ("leads", |p| {
            for kind in [tabs::LeadKind::Line, tabs::LeadKind::Arc] {
                for side in [tabs::LeadSide::Outside, tabs::LeadSide::Inside, tabs::LeadSide::Left] {
                    for (len, ang) in [(2.0, 45.0), (0.0, 0.0), (1e9, 360.0), (2.0, f64::NAN)] {
                        let l = Some(tabs::Lead { kind, length: len, angle_deg: ang, side });
                        let _ = tabs::lead_in_out(p, &tabs::LeadOptions { lead_in: l, lead_out: l });
                    }
                }
            }
        }),
        ("hull", |p| {
            let _ = hull::rubber_band_outline(std::slice::from_ref(p), 0.01);
        }),
        ("clip", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = clip::clip_stroke(p, std::slice::from_ref(&sq), 0.01);
            let _ = clip::clip_stroke(&sq, std::slice::from_ref(p), 0.01);
        }),
        ("slots", |p| {
            let s = slots::find_slots(p, &slots::SlotQuery::default());
            let _ = slots::resize_slots(p, &s, Some(1.0), Some(1.0));
        }),
        ("nest", |p| {
            let _ = nest::nest(&[p.clone(), p.clone()], Rect::new(0.0, 0.0, 100.0, 100.0), &nest::NestOptions::default());
        }),
        ("sheet nest", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let parts = [
                sheet_nest::NestPart::cut(vec![p.clone()]),
                sheet_nest::NestPart { cuts: vec![], marks: vec![p.clone()] },
                sheet_nest::NestPart::cut(vec![p.clone(), sq]),
            ];
            let o = sheet_nest::SheetNestOptions { time_limit: Duration::from_secs(5), max_sheets: 2, ..Default::default() };
            let sheet = Rect::new(0.0, 0.0, 100.0, 100.0);
            let _ = sheet_nest::quick_nest(&parts, &parts[..1], sheet, &o, &mut |_| true);
            let _ = sheet_nest::true_nest(&parts, &parts[..1], sheet, &o, &mut |_| true);
        }),
        ("queries", |p| {
            let sq = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.1);
            let _ = query::point_in_shape(p, Point::new(5.0, 5.0), FillRule::EvenOdd);
            let _ = query::nearest_point(p, Point::new(5.0, 5.0));
            let _ = query::hit_test(p, Point::new(5.0, 5.0), 0.1, true);
            let _ = query::contains(p, &sq, 0.01);
            let _ = query::contains(&sq, p, 0.01);
            let _ = query::containment(&[p.clone(), sq.clone(), p.clone()], 0.01);
            let _ = query::path_intersections(p, &sq);
            let _ = query::path_intersections(p, p);
            let _ = path::area(p, FillRule::EvenOdd, 0.01);
            let _ = path::direction(p);
            let _ = path::set_direction(p, Direction::Clockwise);
            let _ = path::set_start_point(p, Point::new(5.0, 5.0), true);
            let _ = path::reverse(p);
        }),
        ("along path", |p| {
            for mode in [array::AlongMode::Count(5), array::AlongMode::Count(0), array::AlongMode::Spacing(0.0), array::AlongMode::Spacing(1e-12)] {
                let o = array::AlongOptions { mode, rotate_items: true, start_at_nearest: true };
                let _ = array::copy_along_path(Point::new(1.0, 1.0), p, &o);
            }
        }),
        ("warp", |p| {
            let src = path::bounding_box(p).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
            let dst = [Point::new(0.0, 0.0), Point::new(10.0, 1.0), Point::new(12.0, 9.0), Point::new(-1.0, 10.0)];
            let _ = transform::warp_bilinear(p, src, dst, 0.01);
            let _ = transform::warp_perspective(p, src, dst, 0.01);
            let _ = transform::deform_grid(p, src, transform::identity_grid(src), 0.01);
            let _ = transform::mirror_path(p, Point::new(0.0, 0.0), Point::new(0.0, 0.0));
        }),
    ];
    v
}

#[test]
fn degenerate_and_extreme_input_never_panics_or_hangs() {
    let limit = Duration::from_secs(20);
    let mut failures = Vec::new();
    for (op_name, op) in ops() {
        for (in_name, path) in inputs() {
            if std::env::var_os("HUNT_TRACE").is_some() {
                eprintln!("{op_name} on '{in_name}'");
            }
            let (tx, rx) = mpsc::channel();
            let p = path.clone();
            let h = std::thread::Builder::new()
                .stack_size(16 << 20)
                .spawn(move || {
                    let r = std::panic::catch_unwind(|| op(&p));
                    let _ = tx.send(r.map_err(|e| {
                        e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                    }));
                })
                .unwrap();
            match rx.recv_timeout(limit) {
                Ok(Ok(())) => {
                    let _ = h.join();
                }
                Ok(Err(msg)) => failures.push(format!("{op_name} on '{in_name}': panic: {msg}")),
                Err(_) => failures.push(format!("{op_name} on '{in_name}': no result after {limit:?} (thread left running or died)")),
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}
