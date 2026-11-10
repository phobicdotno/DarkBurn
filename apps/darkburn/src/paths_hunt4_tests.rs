//! Bug hunt pass 4, "two ways to the same result": what one path of the app shows or builds
//! must be what the other path builds.

use darkburn_core::ShapeId;
use darkburn_core::kurbo::Point;
use darkburn_core::layer::Mode;
use darkburn_job::Toolpath;

use crate::job_build::*;
use crate::job_field_tests::{input, segs};
use crate::shape_ops::tests::{add_rect, app};
use crate::shell::DarkBurn;

/// An app whose layer 0 cuts lines with automatic tabs.
fn auto_tab_app(spacing: f64, min: u32, max: u32, size: f64) -> DarkBurn {
    let mut a = app();
    let s = a.editor.doc.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    s.tabs.enabled = true;
    s.tabs.auto_spacing = true;
    s.tabs.spacing = spacing;
    s.tabs.min_count = min;
    s.tabs.max_count = max;
    s.tabs.size = size;
    s.tabs.skip_inner = false;
    a
}

fn job(a: &DarkBurn) -> Toolpath {
    build_job_simple(&input(a.editor.doc.clone())).unwrap().toolpath
}

/// The laser-off stretches inside the cuts, as (from, to), joined across vertices.
fn job_gaps(tp: &Toolpath) -> Vec<(Point, Point)> {
    let mut out: Vec<(Point, Point)> = Vec::new();
    for s in segs(tp).iter().filter(|s| s.kind == 'C' && s.power == 0.0) {
        match out.last_mut() {
            Some(g) if g.1.distance(s.from) < 1e-9 => g.1 = s.to,
            _ => out.push((s.from, s.to)),
        }
    }
    out
}

/// The automatic tab marks the Add Tabs tool draws on `ids` (and `tabs.auto` reports) are the
/// gaps the job leaves: as many, at the same places, as long.
fn assert_marks_are_the_job_gaps(a: &DarkBurn, ids: &[ShapeId], what: &str) {
    let marks: Vec<(Point, Point)> =
        ids.iter().flat_map(|id| crate::shape_nodes::auto_gaps(&a.editor.doc, *id, false)).map(|g| (g.from, g.to)).collect();
    let gaps = job_gaps(&job(a));
    assert_eq!(marks.len(), gaps.len(), "{what}: the canvas marks {marks:?}, the job leaves {gaps:?}");
    for (f, t) in &marks {
        let same = |g: &&(Point, Point)| (g.0.distance(*f) < 1e-6 && g.1.distance(*t) < 1e-6) || (g.0.distance(*t) < 1e-6 && g.1.distance(*f) < 1e-6);
        assert!(gaps.iter().any(|g| same(&g)), "{what}: the mark {f:?} to {t:?} is no gap of the job: {gaps:?}");
    }
}

#[test]
fn automatic_tab_marks_match_the_job_in_the_plain_case() {
    let mut a = auto_tab_app(40.0, 1, 10, 2.0);
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    assert_eq!(crate::shape_nodes::auto_gaps(&a.editor.doc, r, false).len(), 4);
    assert_marks_are_the_job_gaps(&a, &[r], "four tabs on a square");
}

#[test]
fn automatic_tab_marks_match_the_job_when_min_tabs_is_above_max_tabs() {
    let mut a = auto_tab_app(40.0, 6, 4, 2.0);
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    assert_marks_are_the_job_gaps(&a, &[r], "min 6, max 4");
}

#[test]
fn automatic_tab_marks_match_the_job_when_max_tabs_is_zero() {
    let mut a = auto_tab_app(40.0, 0, 0, 2.0);
    let r = add_rect(&mut a, 10.0, 10.0, 40.0, 40.0, 0);
    assert_marks_are_the_job_gaps(&a, &[r], "max 0");
}

#[test]
fn automatic_tab_marks_match_the_job_on_a_shape_too_small_for_its_tabs() {
    let mut a = auto_tab_app(1.0, 1, 10, 2.0);
    let r = add_rect(&mut a, 10.0, 10.0, 2.0, 2.0, 0);
    assert_marks_are_the_job_gaps(&a, &[r], "a 2 mm square with 2 mm tabs every millimetre");
}

#[test]
fn automatic_tab_marks_match_the_job_with_skip_inner_shapes_and_one_shape_inside_another() {
    let mut a = auto_tab_app(40.0, 1, 10, 2.0);
    a.editor.doc.layer_mut(0).main_mut().tabs.skip_inner = true;
    let outer = add_rect(&mut a, 0.0, 0.0, 100.0, 100.0, 0);
    let inner = add_rect(&mut a, 30.0, 30.0, 40.0, 40.0, 0);
    assert_marks_are_the_job_gaps(&a, &[outer, inner], "a square inside a square, two shapes");
}
