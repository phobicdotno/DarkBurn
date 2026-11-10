//! Manual tabs and forced start points go document -> command -> `job_build::build_job` ->
//! toolpath here: where the gap or the first move lands, through transforms, groups, copies,
//! undo, save and load, node edits, kerf, lead-ins and the optimiser (LINE-06, PLAN-13, DS-05).

use darkburn_core::kurbo::{Affine, Point, Vec2};
use darkburn_core::layer::Mode;
use darkburn_core::{CutDirection, ShapeId, ShapeKind};
use darkburn_job::{Op, Toolpath};
use serde_json::{Value, json};

use crate::job_build::*;
use crate::job_field_tests::{input, segs};
use crate::shape_ops::tests::{add_rect, app, run, select};
use crate::shell::DarkBurn;

/// An app whose layer 0 cuts lines with manual tabs of 2 mm.
fn tab_app() -> DarkBurn {
    let mut a = app();
    let s = a.editor.doc.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    s.tabs.enabled = true;
    s.tabs.auto_spacing = false;
    s.tabs.size = 2.0;
    a
}

fn job_with(a: &DarkBurn, f: impl FnOnce(&mut BuildInput)) -> Toolpath {
    let mut i = input(a.editor.doc.clone());
    f(&mut i);
    build_job_simple(&i).unwrap().toolpath
}

fn job(a: &DarkBurn) -> Toolpath {
    job_with(a, |_| {})
}

/// The laser-off stretches inside the cuts, as (from, to), joined across vertices.
fn gaps(tp: &Toolpath) -> Vec<(Point, Point)> {
    let mut out: Vec<(Point, Point)> = Vec::new();
    for s in segs(tp).iter().filter(|s| s.kind == 'C' && s.power == 0.0) {
        match out.last_mut() {
            Some(g) if g.1.distance(s.from) < 1e-9 => g.1 = s.to,
            _ => out.push((s.from, s.to)),
        }
    }
    out
}

/// Exactly one 2 mm gap centred on each of `want`, and no others.
fn assert_gaps_at(tp: &Toolpath, want: &[Point], what: &str) {
    let g = gaps(tp);
    assert_eq!(g.len(), want.len(), "{what}: {g:?}");
    for w in want {
        let hit = g.iter().find(|(a, b)| a.midpoint(*b).distance(*w) < 1e-6).unwrap_or_else(|| panic!("{what}: a gap centred on {w:?} in {g:?}"));
        assert!((hit.0.distance(hit.1) - 2.0).abs() < 1e-6, "{what}: a 2 mm gap: {hit:?}");
    }
}

/// Where each cut starts (the rapid before it).
fn starts(tp: &Toolpath) -> Vec<Point> {
    tp.ops.iter().filter_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).collect()
}

/// The first laser-on move of the job.
fn first_cut(tp: &Toolpath) -> (Point, Vec2) {
    let s = segs(tp).into_iter().find(|s| s.kind == 'C' && s.power > 0.0).expect("a cut");
    (s.from, s.to - s.from)
}

fn tab_docs(a: &DarkBurn, id: ShapeId) -> Vec<Point> {
    let s = a.editor.doc.get(id).unwrap();
    s.tabs.iter().map(|t| s.transform * Point::new(t[0], t[1])).collect()
}

fn start_docs(a: &DarkBurn, id: ShapeId) -> Vec<Point> {
    let s = a.editor.doc.get(id).unwrap();
    s.start_points().map(|t| s.transform * Point::new(t[0], t[1])).collect()
}

fn transform(a: &mut DarkBurn, id: ShapeId, t: Affine) {
    a.editor.edit("Transform", |d, _| {
        let s = d.get_mut(id).unwrap();
        s.transform = t * s.transform;
        true
    });
}

// ---------------------------------------------------------------- tabs

#[test]
fn a_tab_placed_by_command_is_a_gap_at_that_spot_whatever_the_planner_does() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.3 })).unwrap();
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 50.0, "y": 20.0 })).unwrap();
    let want = [Point::new(30.0, 10.0), Point::new(50.0, 20.0)];
    assert_gaps_at(&job(&a), &want, "optimised");
    assert_gaps_at(&job_with(&a, |i| i.settings.planner.optimize = false), &want, "drawn order");
    // A forced start point and direction move the cut, not the tabs.
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 50.5, "y": 21.5 })).unwrap();
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "clockwise" })).unwrap();
    let tp = job(&a);
    assert!(starts(&tp)[0].distance(Point::new(50.0, 21.5)) < 1e-6);
    assert_gaps_at(&tp, &want, "start next to a tab");
    // Tabs off on the layer: a solid cut, and the tabs are kept for when they come back on.
    a.editor.doc.layer_mut(0).main_mut().tabs.enabled = false;
    assert!(gaps(&job(&a)).is_empty());
    assert_eq!(a.editor.doc.get(r).unwrap().tabs.len(), 2);
}

#[test]
fn the_preview_shows_the_tab_gaps() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    let built = std::sync::Arc::new(build_job_simple(&input(a.editor.doc.clone())).unwrap());
    let p = crate::preview::PreviewData::new(built, &AdditionalSettings::default(), 0);
    // What the preview window draws: the piece over the tab is there with the laser off.
    let off: f64 = p.segs.iter().filter(|s| !s.rapid && s.power == 0.0).map(|s| s.a.distance(s.b)).sum();
    let on: f64 = p.segs.iter().filter(|s| !s.rapid && s.power > 0.0).map(|s| s.a.distance(s.b)).sum();
    assert!((off - 2.0).abs() < 1e-6 && (on - 158.0).abs() < 1e-6, "off {off}, on {on}");
    let piece = p.segs.iter().find(|s| !s.rapid && s.power == 0.0).unwrap();
    assert!(piece.a.midpoint(piece.b).distance(Point::new(30.0, 10.0)) < 1e-6, "{piece:?}");
}

#[test]
fn tabs_follow_the_shape_through_transforms_groups_and_ungrouping() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 20.0, 0);
    let other = add_rect(&mut a, 100.0, 100.0, 10.0, 10.0, 0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 20.0, "y": 10.0 })).unwrap();
    let p0 = Point::new(20.0, 10.0);
    // Rotate, mirror and stretch the shape: the tab stays on the same spot of the outline.
    let t1 = Affine::translate((80.0, 5.0)) * Affine::rotate(0.7) * Affine::scale_non_uniform(-1.0, 1.5);
    transform(&mut a, r, t1);
    assert_gaps_at(&job(&a), &[t1 * p0], "transformed");
    // In a group that is moved and turned.
    select(&mut a, &[r, other]);
    run(&mut a, "arrange.group", Value::Null).unwrap();
    let g = a.editor.doc.iter().find(|s| s.is_group()).expect("a group").id;
    let t2 = Affine::translate((-20.0, 30.0)) * Affine::rotate(-0.3);
    transform(&mut a, g, t2);
    assert_gaps_at(&job(&a), &[t2 * t1 * p0], "grouped");
    // And out of it again.
    select(&mut a, &[g]);
    run(&mut a, "arrange.ungroup", Value::Null).unwrap();
    assert!(a.editor.doc.iter().all(|s| !s.is_group()));
    assert_gaps_at(&job(&a), &[t2 * t1 * p0], "ungrouped");
}

#[test]
fn tabs_and_start_points_survive_copy_paste_and_duplicate() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 10.0, "y": 25.0 })).unwrap();
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "clockwise" })).unwrap();
    select(&mut a, &[r]);
    run(&mut a, "edit.copy", Value::Null).unwrap();
    run(&mut a, "edit.paste_in_place", Value::Null).unwrap();
    select(&mut a, &[r]);
    run(&mut a, "edit.duplicate", Value::Null).unwrap();
    let ids: Vec<ShapeId> = a.editor.doc.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 3);
    let orig = a.editor.doc.get(r).unwrap().clone();
    for (k, id) in ids.iter().enumerate() {
        let s = a.editor.doc.get(*id).unwrap();
        assert_eq!((&s.tabs, s.start_point, s.direction), (&orig.tabs, orig.start_point, CutDirection::Clockwise), "copy {k}");
        // Spread the copies out so that each is cut on its own.
        let bb = s.bbox();
        transform(&mut a, *id, Affine::translate((100.0 * k as f64 - (bb.x0 - 10.0), 10.0 - bb.y0)));
    }
    let tp = job_with(&a, |i| i.device = JobDevice::new(400.0, 400.0));
    let want: Vec<Point> = (0..3).map(|k| Point::new(30.0 + 100.0 * k as f64, 10.0)).collect();
    assert_gaps_at(&tp, &want, "three copies");
    let st = starts(&tp);
    for k in 0..3 {
        assert!(st.iter().any(|p| p.distance(Point::new(10.0 + 100.0 * k as f64, 25.0)) < 1e-6), "copy {k} starts at its start point: {st:?}");
    }
}

#[test]
fn tabs_and_start_points_undo_and_redo() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    let plain = job(&a);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    let tabbed = job(&a);
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 50.0, "y": 30.0 })).unwrap();
    let started = job(&a);
    assert!(plain != tabbed && tabbed != started);
    assert!(a.editor.undo());
    assert!(a.editor.doc.get(r).unwrap().start_point.is_none());
    assert_eq!(job(&a), tabbed);
    assert!(a.editor.undo());
    assert!(a.editor.doc.get(r).unwrap().tabs.is_empty());
    assert_eq!(job(&a), plain);
    assert!(a.editor.redo());
    assert_eq!(job(&a), tabbed);
    assert!(a.editor.redo());
    assert_eq!(job(&a), started);
    // Removing and clearing are steps too.
    run(&mut a, "tabs.remove", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    run(&mut a, "shape.clear_start_point", json!({ "shape": r.0 })).unwrap();
    assert_eq!(job(&a), plain);
    assert!(a.editor.undo() && a.editor.undo());
    assert_eq!(job(&a), started);
}

#[test]
fn tabs_and_start_points_survive_save_and_open() {
    let dir = std::env::temp_dir().join(format!("darkburn-jobplan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("tabs.dbrn");
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    transform(&mut a, r, Affine::rotate_about(0.4, Point::new(30.0, 30.0)));
    let c = run(&mut a, "shape.add_path", json!({ "svg_path": "M100 0 L120 0 L120 20 L100 20 Z M150 0 L170 0 L170 20 L150 20 Z M100 40 L170 40" }))
        .unwrap();
    let c = ShapeId(c["id"].as_u64().unwrap());
    let on_rect = a.editor.doc.get(r).unwrap().transform * Point::new(0.0, -20.0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": on_rect.x, "y": on_rect.y })).unwrap();
    let start = a.editor.doc.get(r).unwrap().transform * Point::new(20.0, 5.0);
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": start.x, "y": start.y })).unwrap();
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "counter_clockwise" })).unwrap();
    run(&mut a, "tabs.add", json!({ "shape": c.0, "x": 160.0, "y": 20.0 })).unwrap();
    run(&mut a, "tabs.add", json!({ "shape": c.0, "x": 130.0, "y": 40.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "shape": c.0, "x": 110.0, "y": 20.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "shape": c.0, "x": 170.0, "y": 10.0 })).unwrap();
    let before = job(&a);
    assert_eq!(gaps(&before).len(), 3);
    crate::files::save_to(&mut a, &file).unwrap();
    let mut b = app();
    crate::files::open_path(&mut b, &file).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    for id in [r, c] {
        let (x, y) = (a.editor.doc.get(id).unwrap(), b.editor.doc.get(id).unwrap());
        assert_eq!((&x.tabs, x.start_points().collect::<Vec<_>>(), x.direction), (&y.tabs, y.start_points().collect::<Vec<_>>(), y.direction));
    }
    assert_eq!(b.editor.doc.get(c).unwrap().start_points().count(), 2);
    assert_eq!(job(&b), before, "the opened project cuts exactly the same");
}

#[test]
fn a_tab_stays_on_the_outline_through_node_edits_and_conversion() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    transform(&mut a, r, Affine::rotate_about(std::f64::consts::FRAC_PI_2, Point::new(30.0, 30.0)));
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    select(&mut a, &[r]);
    run(&mut a, "edit.convert_path", Value::Null).unwrap();
    assert!(matches!(a.editor.doc.get(r).unwrap().kind, ShapeKind::Path { .. }));
    assert_gaps_at(&job(&a), &[Point::new(30.0, 10.0)], "converted to a path");
    // Pull every node of the bottom edge down by 6 mm: the tab goes down with its edge.
    let nodes: Vec<(usize, Point)> = a.editor.doc.get(r).unwrap().nodes().into_iter().enumerate().collect();
    let bottom: Vec<usize> = nodes.iter().filter(|(_, p)| (p.y - 10.0).abs() < 1e-6).map(|(i, _)| *i).collect();
    assert_eq!(bottom.len(), 2, "{nodes:?}");
    for n in bottom {
        run(&mut a, "node.edit", json!({ "op": "move", "subpath": 0, "node": n, "dx": 0, "dy": -6 })).unwrap();
    }
    let t = tab_docs(&a, r);
    assert_eq!(t.len(), 1);
    assert!((t[0].y - 4.0).abs() < 1e-6 && (t[0].x - 30.0).abs() < 1.0, "the stored tab moved onto the new outline: {t:?}");
    assert_gaps_at(&job(&a), &t, "after the node edit");
    // A new node next to it changes nothing.
    run(&mut a, "node.edit", json!({ "op": "insert", "subpath": 0, "seg": 0, "t": 0.5 })).unwrap();
    assert_eq!(tab_docs(&a, r).len(), 1);
    assert!(tab_docs(&a, r)[0].distance(t[0]) < 1e-6);
    assert_gaps_at(&job(&a), &t, "after inserting a node");
}

#[test]
fn tabs_on_open_lines_and_on_one_sub_path_of_a_compound_shape() {
    let mut a = tab_app();
    let l = run(&mut a, "shape.add_line", json!({ "points": [[0, 0], [40, 0], [40, 30]], "layer": 0 })).unwrap();
    let l = ShapeId(l["id"].as_u64().unwrap());
    run(&mut a, "tabs.add", json!({ "shape": l.0, "x": 15.0, "y": 0.4 })).unwrap();
    run(&mut a, "tabs.add", json!({ "shape": l.0, "x": 40.0, "y": 12.0 })).unwrap();
    let want = [Point::new(15.0, 0.0), Point::new(40.0, 12.0)];
    assert_gaps_at(&job(&a), &want, "open line");
    // Cut from the other end, the gaps are the same.
    run(&mut a, "shape.set_direction", json!({ "shape": l.0, "direction": "reversed" })).unwrap();
    let tp = job(&a);
    assert!(starts(&tp)[0].distance(Point::new(40.0, 30.0)) < 1e-6);
    assert_gaps_at(&tp, &want, "open line, reversed");
    // A frame (outline and hole in one shape): the tab is cut out of the hole only.
    let f = run(&mut a, "shape.add_path", json!({ "svg_path": "M100 0 L160 0 L160 60 L100 60 Z M120 20 L140 20 L140 40 L120 40 Z", "layer": 0 }))
        .unwrap();
    let f = ShapeId(f["id"].as_u64().unwrap());
    run(&mut a, "tabs.add", json!({ "shape": f.0, "x": 130.0, "y": 21.0 })).unwrap();
    let mut all = want.to_vec();
    all.push(Point::new(130.0, 20.0));
    assert_gaps_at(&job(&a), &all, "compound shape");
    // Skip inner shapes is about the automatic tabs: the one placed by hand stays.
    a.editor.doc.layer_mut(0).main_mut().tabs.skip_inner = true;
    assert_gaps_at(&job(&a), &all, "skip inner shapes");
}

#[test]
fn manual_tabs_replace_the_automatic_ones_of_their_shape_in_the_job_and_on_the_canvas() {
    let mut a = tab_app();
    {
        let t = &mut a.editor.doc.layer_mut(0).main_mut().tabs;
        t.auto_spacing = true;
        t.spacing = 40.0;
        t.min_count = 1;
        t.max_count = 10;
    }
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    let auto = crate::shape_nodes::auto_gaps(&a.editor.doc, r, false);
    assert_eq!(auto.len(), 4);
    // The job leaves the automatic gaps where the Add Tabs tool draws them, wherever it starts.
    let marks: Vec<Point> = auto.iter().map(|g| g.from.midpoint(g.to)).collect();
    assert_gaps_at(&job(&a), &marks, "automatic");
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 23.0, "y": 50.0 })).unwrap();
    assert_gaps_at(&job(&a), &marks, "automatic, forced start");
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 12.0, "y": 10.0 })).unwrap();
    assert!(crate::shape_nodes::auto_gaps(&a.editor.doc, r, false).is_empty(), "no automatic marks on a shape with manual tabs");
    assert_gaps_at(&job(&a), &[Point::new(12.0, 10.0)], "manual");
    run(&mut a, "tabs.clear", json!({ "shape": r.0 })).unwrap();
    assert_gaps_at(&job(&a), &marks, "automatic again");
}

#[test]
fn partly_cut_tabs_are_pulsed_with_the_devices_tab_pulse_width() {
    let mut a = tab_app();
    {
        let t = &mut a.editor.doc.layer_mut(0).main_mut().tabs;
        t.partial = true;
        t.tab_power = 10.0;
    }
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 30.0, "y": 10.0 })).unwrap();
    let on_in_tab = |tp: &Toolpath| -> Vec<(f64, f64)> {
        segs(tp)
            .iter()
            .filter(|s| {
                s.kind == 'C' && s.from.y == 10.0 && s.to.y == 10.0 && s.from.x.min(s.to.x) >= 29.0 - 1e-9 && s.from.x.max(s.to.x) <= 31.0 + 1e-9
            })
            .map(|s| (s.from.distance(s.to), s.power))
            .collect()
    };
    // No pulse width: one piece at the tab power.
    let plain = on_in_tab(&job(&a));
    assert_eq!(plain.len(), 1, "{plain:?}");
    assert!((plain[0].0 - 2.0).abs() < 1e-9 && plain[0].1 == 10.0, "{plain:?}");
    // The device setting (DS-05) reaches the job: 0.1 mm full-power pulses, a fifth of the time.
    let mut dev = crate::devices::Device::default();
    dev.settings.tab_pulse_width = 0.1;
    assert_eq!(dev.job_device().tab_pulse_width, 0.1);
    let pulsed = on_in_tab(&job_with(&a, |i| i.device = JobDevice { tab_pulse_width: 0.1, ..i.device }));
    assert!(pulsed.iter().all(|(_, p)| *p == 0.0 || *p == 50.0), "only on and off: {pulsed:?}");
    // (The last pulse runs on into the cut after the tab, so count the pauses.)
    let off: Vec<f64> = pulsed.iter().filter(|(_, p)| *p == 0.0).map(|(l, _)| *l).collect();
    assert_eq!(off.len(), 4, "four pulses in a 2 mm tab: {pulsed:?}");
    assert!(off.iter().all(|l| (l - 0.4).abs() < 1e-9), "0.1 mm on, 0.4 mm off: {pulsed:?}");
    // Tabs that are skipped altogether are not pulsed.
    a.editor.doc.layer_mut(0).main_mut().tabs.partial = false;
    assert_gaps_at(&job_with(&a, |i| i.device = JobDevice { tab_pulse_width: 0.1, ..i.device }), &[Point::new(30.0, 10.0)], "laser off");
}

// ---------------------------------------------------------------- start points

#[test]
fn the_start_point_command_sets_where_and_which_way_a_closed_shape_is_cut() {
    let mut a = tab_app();
    let far = add_rect(&mut a, 150.0, 150.0, 10.0, 10.0, 0);
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    let e = run(&mut a, "shape.add_ellipse", json!({ "x": 80.0, "y": 10.0, "width": 40.0, "height": 20.0, "layer": 0 })).unwrap();
    let e = ShapeId(e["id"].as_u64().unwrap());
    let _ = far;
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 51.0, "y": 37.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "shape": e.0, "x": 100.0, "y": 31.0 })).unwrap();
    assert!(matches!(a.editor.doc.get(r).unwrap().kind, ShapeKind::Rect { .. }), "still a rectangle");
    assert!(start_docs(&a, r)[0].distance(Point::new(50.0, 37.0)) < 1e-9, "stored on the outline");
    for bits in 0..16u32 {
        let on = |b: u32| bits & (1 << b) != 0;
        let tp = job_with(&a, |i| {
            i.settings.planner.optimize = on(0);
            i.settings.planner.best_start = on(1);
            i.settings.planner.best_direction = on(2);
            i.settings.planner.prefer_corners = on(3);
        });
        let st = starts(&tp);
        assert_eq!(st.len(), 3);
        assert!(st.iter().any(|p| p.distance(Point::new(50.0, 37.0)) < 1e-6), "rectangle, options {bits:04b}: {st:?}");
        assert!(st.iter().any(|p| p.distance(Point::new(100.0, 30.0)) < 1e-3), "ellipse, options {bits:04b}: {st:?}");
    }
    // The direction from there.
    let heading = |a: &DarkBurn| {
        let tp = job(a);
        let s =
            segs(&tp).into_iter().find(|s| s.kind == 'C' && s.from.distance(Point::new(50.0, 37.0)) < 1e-6).expect("the cut from the start point");
        s.to - s.from
    };
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "clockwise" })).unwrap();
    assert!(heading(&a).y < 0.0, "clockwise goes down the right edge");
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "counter_clockwise" })).unwrap();
    assert!(heading(&a).y > 0.0);
    // The mirrored shape still turns the way that was asked for.
    transform(&mut a, r, Affine::translate((60.0, 0.0)) * Affine::scale_non_uniform(-1.0, 1.0));
    let tp = job(&a);
    let s = segs(&tp)
        .into_iter()
        .find(|s| s.kind == 'C' && s.from.distance(Point::new(10.0, 37.0)) < 1e-6)
        .expect("the start point was mirrored with the shape");
    assert!((s.to - s.from).y < 0.0, "counter-clockwise goes down the left edge");
    // Cleared: the planner is free again.
    run(&mut a, "shape.clear_start_point", json!({ "shape": r.0 })).unwrap();
    assert!(start_docs(&a, r).is_empty());
    assert!(starts(&job(&a)).iter().all(|p| p.distance(Point::new(10.0, 37.0)) > 1.0));
}

#[test]
fn a_start_point_holds_with_kerf_offset_and_lead_in() {
    let mut a = tab_app();
    {
        let s = a.editor.doc.layer_mut(0).main_mut();
        s.tabs.enabled = false;
        s.kerf = 0.5;
    }
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    run(&mut a, "shape.set_start_point", json!({ "shape": r.0, "x": 50.0, "y": 30.0 })).unwrap();
    // The outline that is cut is moved by the kerf; the cut starts at its point nearest the start point.
    let moved = (first_cut(&job(&a)).0 - Point::new(50.0, 30.0)).hypot();
    assert!((moved - 0.5).abs() < 1e-6, "half a millimetre from the start point: {moved}");
    let (p, _) = first_cut(&job(&a));
    assert!((p.y - 30.0).abs() < 1e-6, "{p:?}");
    for (dir, down) in [("clockwise", true), ("counter_clockwise", false)] {
        run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": dir })).unwrap();
        let (q, h) = first_cut(&job(&a));
        assert!(q.distance(p) < 1e-6 && (h.y < 0.0) == down, "{dir}: {q:?} {h:?}");
    }
    // Reversed is the other way round from the drawn direction, kerf or not.
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "auto" })).unwrap();
    let drawn = first_cut(&job_with(&a, |i| i.settings.planner.best_direction = false)).1;
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "reversed" })).unwrap();
    let rev = first_cut(&job(&a)).1;
    assert!(drawn.dot(rev) < 0.0, "{drawn:?} against {rev:?}");
    a.editor.doc.layer_mut(0).main_mut().kerf = 0.0;
    let rev0 = first_cut(&job(&a)).1;
    assert!(rev0.dot(rev) > 0.0, "the kerf offset keeps the direction");
    // A lead-in ends on the start point and the outline goes on from there.
    {
        let s = a.editor.doc.layer_mut(0).main_mut();
        s.lead_in.enabled = true;
        s.lead_in.length = 3.0;
        s.lead_in.angle = 90.0;
    }
    run(&mut a, "shape.set_direction", json!({ "shape": r.0, "direction": "clockwise" })).unwrap();
    let tp = job(&a);
    let s = segs(&tp);
    let i = s.iter().position(|s| s.kind == 'C').unwrap();
    assert!(
        s[i].from.distance(Point::new(53.0, 30.0)) < 1e-6 && s[i].to.distance(Point::new(50.0, 30.0)) < 1e-6,
        "lead-in from the waste side: {:?}",
        s[i]
    );
    assert!(s[i + 1].to.distance(Point::new(50.0, 10.0)) < 1e-6, "then clockwise: {:?}", s[i + 1]);
}

#[test]
fn every_sub_path_of_a_compound_shape_keeps_its_own_start_point() {
    let mut a = tab_app();
    let c = run(&mut a, "shape.add_path", json!({ "svg_path": "M0 0 L20 0 L20 20 L0 20 Z M50 0 L70 0 L70 20 L50 20 Z M100 0 L100 20", "layer": 0 }))
        .unwrap();
    let c = ShapeId(c["id"].as_u64().unwrap());
    select(&mut a, &[c]);
    run(&mut a, "shape.set_start_point", json!({ "x": 10.0, "y": 21.0 })).unwrap();
    run(&mut a, "shape.set_start_point", json!({ "x": 70.0, "y": 8.0 })).unwrap();
    let pts = start_docs(&a, c);
    assert_eq!(pts.len(), 2, "one for each closed sub-path: {pts:?}");
    assert!(pts.contains(&Point::new(10.0, 20.0)) && pts.contains(&Point::new(70.0, 8.0)), "{pts:?}");
    let both = |a: &DarkBurn, p: Point, q: Point| {
        for optimize in [true, false] {
            let st = starts(&job_with(a, |i| i.settings.planner.optimize = optimize));
            assert!(st.iter().any(|s| s.distance(p) < 1e-6) && st.iter().any(|s| s.distance(q) < 1e-6), "optimize {optimize}: {st:?}");
        }
    };
    both(&a, Point::new(10.0, 20.0), Point::new(70.0, 8.0));
    // Clicking a sub-path again moves its start point; the other one stays.
    run(&mut a, "shape.set_start_point", json!({ "x": -1.0, "y": 5.0 })).unwrap();
    assert_eq!(start_docs(&a, c).len(), 2);
    both(&a, Point::new(0.0, 5.0), Point::new(70.0, 8.0));
    // The markers of the Start Points tool show the same starts.
    let m = crate::shape_nodes::marker_path(&a.editor.doc, c).unwrap();
    let ms: Vec<Point> = darkburn_geom::path::contours(&m).iter().filter_map(|c| c.start()).collect();
    assert_eq!(&ms[..2], &[Point::new(0.0, 5.0), Point::new(70.0, 8.0)]);
    // With `all` every closed sub-path starts at its point nearest the click.
    run(&mut a, "shape.set_start_point", json!({ "x": 35.0, "y": 10.0, "all": true })).unwrap();
    assert_eq!(start_docs(&a, c).len(), 2);
    both(&a, Point::new(20.0, 10.0), Point::new(50.0, 10.0));
    // An open line has no start point to choose.
    let l = run(&mut a, "shape.add_line", json!({ "points": [[200, 0], [220, 0]], "layer": 0 })).unwrap();
    let l = ShapeId(l["id"].as_u64().unwrap());
    let out = run(&mut a, "shape.set_start_point", json!({ "shape": l.0, "x": 220.0, "y": 0.0 })).unwrap();
    assert_eq!(out["changed"], false, "{out}");
    assert!(start_docs(&a, l).is_empty());
    // Clearing takes all of them away; undo brings them back.
    run(&mut a, "shape.clear_start_point", json!({ "shape": c.0 })).unwrap();
    assert!(start_docs(&a, c).is_empty());
    assert!(a.editor.undo());
    assert_eq!(start_docs(&a, c).len(), 2);
}

#[test]
fn shapes_report_tabs_start_points_and_direction_to_agents() {
    let mut a = tab_app();
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    transform(&mut a, r, Affine::translate((5.0, 0.0)));
    run(&mut a, "tabs.add", json!({ "shape": r.0, "x": 35.0, "y": 10.0 })).unwrap();
    select(&mut a, &[r]);
    run(&mut a, "shape.set_props", json!({ "direction": "clockwise", "start_point": [55.0, 30.0] })).unwrap();
    let j = crate::mcp_inspect::shape_json(a.editor.doc.get(r).unwrap(), true);
    assert_eq!(j["tabs"], 1);
    assert_eq!(j["tab_points"], json!([[35.0, 10.0]]));
    assert_eq!(j["start_points"], json!([[55.0, 30.0]]));
    assert_eq!(j["direction"], "clockwise");
    assert!(starts(&job(&a))[0].distance(Point::new(55.0, 30.0)) < 1e-6);
    // The same commands through the MCP backend, as an agent calls them.
    {
        use darkburn_mcp::Backend;
        let shared = std::rc::Rc::new(std::cell::RefCell::new(tab_app()));
        let mut b = crate::mcp_backend::McpBackend::new(shared.clone(), egui::Context::default(), true);
        let id = b.execute("shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 40, "layer": 0 })).unwrap()["id"].as_u64().unwrap();
        b.execute("shape.set_start_point", json!({ "shape": id, "x": 30.0, "y": 51.0 })).unwrap();
        b.execute("shape.set_direction", json!({ "shape": id, "direction": "counter_clockwise" })).unwrap();
        b.execute("tabs.add", json!({ "shape": id, "x": 10.0, "y": 30.0 })).unwrap();
        let app = shared.borrow();
        let tp = job(&app);
        assert!(starts(&tp)[0].distance(Point::new(30.0, 50.0)) < 1e-6);
        assert!(first_cut(&tp).1.x < 0.0, "counter-clockwise along the top edge heads left");
        assert_gaps_at(&tp, &[Point::new(10.0, 30.0)], "placed by an agent");
    }
    run(&mut a, "shape.set_props", json!({ "start_point": null, "direction": "auto" })).unwrap();
    let j = crate::mcp_inspect::shape_json(a.editor.doc.get(r).unwrap(), true);
    assert_eq!(j["start_points"], json!([]));
    assert_eq!(j["direction"], "auto");
}
