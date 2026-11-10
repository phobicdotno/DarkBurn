//! What the shape operations must keep: nothing lost, no winding turned round.

use darkburn_geom::kurbo::{BezPath, Circle, Rect, Shape};
use darkburn_geom::{FillRule, boolean, cleanup, offset, path};

fn svg(s: &str) -> BezPath {
    BezPath::from_svg(s).unwrap()
}

fn area(p: &BezPath) -> f64 {
    path::area(p, FillRule::EvenOdd, 0.001)
}

fn area_all(ps: &[BezPath]) -> f64 {
    ps.iter().map(area).sum()
}

fn valid() -> Vec<(&'static str, BezPath, f64)> {
    vec![
        ("square", Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.1), 100.0),
        ("clockwise square", svg("M0 0 L0 10 L10 10 L10 0 Z"), 100.0),
        ("duplicate nodes", svg("M0 0 L0 0 L10 0 L10 0 L10 10 L10 10 L0 10 L0 10 Z"), 100.0),
        ("collinear nodes", svg("M0 0 L5 0 L10 0 L10 5 L10 10 L5 10 L0 10 L0 5 Z"), 100.0),
        ("two squares at a corner", svg("M0 0 L10 0 L10 10 L0 10 Z M10 10 L20 10 L20 20 L10 20 Z"), 200.0),
        ("one outline through its own corner", svg("M0 0 L10 0 L10 10 L20 10 L20 20 L10 20 L10 10 L0 10 Z"), 200.0),
        ("frame", svg("M0 0 L20 0 L20 20 L0 20 Z M5 5 L5 15 L15 15 L15 5 Z"), 300.0),
        ("frame same winding", svg("M0 0 L20 0 L20 20 L0 20 Z M5 5 L15 5 L15 15 L5 15 Z"), 300.0),
        ("open ends meet", svg("M0 0 L10 0 L10 10 L0 10 L0 0"), 100.0),
        ("circle", Circle::new((5.0, 5.0), 5.0).to_path(1e-6), std::f64::consts::PI * 25.0),
        ("tiny square", Rect::new(0.0, 0.0, 0.01, 0.01).to_path(0.1), 1e-4),
        ("far square", Rect::new(1e6, 1e6, 1e6 + 10.0, 1e6 + 10.0).to_path(0.1), 100.0),
        ("spike", svg("M0 0 L10 0 L10 10 L5 10 L5 30 L5 10 L0 10 Z"), 100.0),
    ]
}

fn garbage() -> Vec<BezPath> {
    vec![
        BezPath::new(),
        svg("M5 5"),
        svg("M5 5 L5 5 Z"),
        svg("M0 0 L10 0 L20 0 Z"),
        svg("M100 100 L110 100 L100 100 Z"),
        {
            let mut p = BezPath::new();
            p.move_to((0.0, 0.0));
            p.line_to((f64::NAN, 5.0));
            p.line_to((3.0, 3.0));
            p.close_path();
            p
        },
        svg("M-1e308 -1e308 L1e308 1e308 L1e308 -1e308 Z"),
    ]
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-3 * b.abs().max(1e-6)
}

#[test]
fn booleans_keep_a_good_shape_whatever_rubbish_is_selected_with_it() {
    let mut wrong = Vec::new();
    for (name, p, want) in valid() {
        for (gi, g) in garbage().iter().enumerate() {
            for rule in [FillRule::EvenOdd, FillRule::NonZero] {
                // A same-winding hole is filled by the nonzero rule.
                let want = if rule == FillRule::NonZero && name == "frame same winding" { 400.0 } else { want };
                let u = boolean::union(&[p.clone(), g.clone()], rule, 0.001);
                if !close(area_all(&u), want) {
                    wrong.push(format!("union of {name} and rubbish {gi} ({rule:?}): area {} not {want}", area_all(&u)));
                }
                let w = boolean::weld(&[g.clone(), p.clone()], rule, 0.001);
                if !close(area(&w), want) {
                    wrong.push(format!("weld of rubbish {gi} and {name} ({rule:?}): area {} not {want}", area(&w)));
                }
                let d = boolean::difference(std::slice::from_ref(&p), std::slice::from_ref(g), rule, 0.001);
                if !close(area_all(&d), want) {
                    wrong.push(format!("{name} minus rubbish {gi} ({rule:?}): area {} not {want}", area_all(&d)));
                }
                for piece in u.iter().chain(&d) {
                    let outer = path::contours(piece).into_iter().next().map(|c| c.signed_area()).unwrap_or(1.0);
                    if outer <= 0.0 {
                        wrong.push(format!("{name} with rubbish {gi} ({rule:?}): the outer contour of a result runs clockwise"));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn clean_up_tools_keep_area_closedness_and_direction() {
    let mut wrong = Vec::new();
    for (name, p, want) in valid() {
        let dir = |q: &BezPath| path::contours(q).iter().map(|c| c.signed_area().signum() as i32).collect::<Vec<_>>();
        for tol in [0.001, 0.01, 0.05] {
            let (o, _) = cleanup::optimize(&p, tol);
            if !((area(&o) - want).abs() <= want * 0.02 + 1e-9) {
                wrong.push(format!("optimize {name} at {tol}: area {} not {want}", area(&o)));
            }
            if path::contours(&o).len() != path::contours(&p).len() {
                wrong.push(format!("optimize {name} at {tol}: {} subpaths became {}", path::contours(&p).len(), path::contours(&o).len()));
            } else if dir(&o) != dir(&p) {
                wrong.push(format!("optimize {name} at {tol}: direction {:?} became {:?}", dir(&p), dir(&o)));
            }
            if path::contours(&o).iter().zip(path::contours(&p)).any(|(a, b)| a.is_closed_within(1e-6) != b.is_closed_within(1e-6)) {
                wrong.push(format!("optimize {name} at {tol}: closed and open changed"));
            }
        }
        let c = cleanup::close_path(&p);
        if !path::contours(&c).iter().all(|c| c.closed) || !close(area(&c), want) || dir(&c) != dir(&p) {
            wrong.push(format!("close_path {name}: area {} dir {:?}", area(&c), dir(&c)));
        }
        let parts = cleanup::break_apart(&p, true, 0.001);
        if !close(area_all(&parts), want) {
            wrong.push(format!("break_apart {name}: area {} not {want}", area_all(&parts)));
        }
        let all = cleanup::break_apart(&p, false, 0.001);
        if all.len() != path::contours(&p).len() {
            wrong.push(format!("break_apart {name}: {} pieces of {} subpaths", all.len(), path::contours(&p).len()));
        }
        // Break every outline into its single lines, then auto-join: the same area comes back.
        let mut lines = Vec::new();
        for c in path::contours(&path::flatten(&p, 0.001)) {
            for s in &c.segs {
                lines.push(path::Contour::new(vec![*s], false).to_bezpath());
            }
        }
        if name != "two squares at a corner" && name != "one outline through its own corner" && name != "spike" {
            let joined = cleanup::auto_join(&lines, 1e-6, &mut |_, _| true).unwrap();
            let total: f64 = joined.iter().map(|j| path::signed_area(j).abs()).sum();
            let raw: f64 = path::contours(&p).iter().map(|c| c.signed_area().abs()).sum();
            if !close(total, raw) || joined.len() != path::contours(&p).len() {
                wrong.push(format!("auto_join {name}: {} pieces with area {total}, wanted {} with {raw}", joined.len(), path::contours(&p).len()));
            }
        }
        let k = offset::kerf_offset(&p, 0.1, 0.001);
        if path::contours(&k).len() != path::contours(&p).len() && name != "tiny square" && name != "spike" && !name.contains("corner") {
            wrong.push(format!("kerf {name}: {} subpaths became {}", path::contours(&p).len(), path::contours(&k).len()));
        } else if path::contours(&k).len() == path::contours(&p).len() && dir(&k) != dir(&p) {
            wrong.push(format!("kerf {name}: direction {:?} became {:?}", dir(&p), dir(&k)));
        }
        let grown = offset::offset(std::slice::from_ref(&p), &offset::OffsetOptions { distance: 0.5, tol: 0.001, ..Default::default() });
        if !(area_all(&grown.outward) > want) {
            wrong.push(format!("offset {name} outward by 0.5: area {} is not above {want}", area_all(&grown.outward)));
        }
    }
    assert!(wrong.is_empty(), "{}:\n{}", wrong.len(), wrong.join("\n"));
}
