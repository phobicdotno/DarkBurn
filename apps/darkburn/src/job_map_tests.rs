//! Jobs moved by print and cut or a rotary set-up, and the kerf direction with "Cut selected
//! graphics": pictures follow the same mapping as the outlines of the job, and a selected hole
//! is still a hole when the outline around it is not selected.

use darkburn_core::Document;
use darkburn_core::kurbo::{Affine, Point, Rect};
use darkburn_core::layer::Mode;
use darkburn_vision::printcut::{PnCFit, PnCMode, ResidualReport};

use crate::job_build::*;
use crate::job_field_tests::{image_doc, input, sq};
use crate::vision_state::VisionState;

fn with_vision(d: &mut Document, f: impl Fn(&mut VisionState)) {
    let mut s = VisionState::default();
    f(&mut s);
    d.vision_settings = Some(s.to_value());
}

fn fit(affine: Affine) -> PnCFit {
    PnCFit {
        mode: PnCMode::Scaled,
        affine,
        scale: 1.0,
        rotation_deg: 0.0,
        translation: (0.0, 0.0),
        residuals: ResidualReport { per_target: Vec::new(), rms_mm: 0.0, max_mm: 0.0 },
    }
}

/// A black picture at (5, 5) to (15, 15) and, on layer 0, a square drawn over the same place.
fn picture_and_outline() -> (Document, Document) {
    let pic = image_doc(|_| {}, vec![0; 16]);
    let mut outline = Document::new();
    let s = outline.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    sq(&mut outline, 0, 5.0, 5.0, 10.0);
    (pic, outline)
}

#[track_caller]
fn near(a: Rect, b: Rect, tol: f64) {
    let d = [a.x0 - b.x0, a.y0 - b.y0, a.x1 - b.x1, a.y1 - b.y1];
    assert!(d.iter().all(|v| v.abs() < tol), "{a:?} is not {b:?}");
}

#[test]
fn print_and_cut_moves_pictures_with_the_outlines() {
    let (mut pic, mut outline) = picture_and_outline();
    let m = Affine::translate((40.0, 25.0)) * Affine::rotate(0.3);
    with_vision(&mut pic, |s| s.pnc.fit = Some(fit(m)));
    with_vision(&mut outline, |s| s.pnc.fit = Some(fit(m)));
    let p = build_job_simple(&input(pic)).unwrap();
    let o = build_job_simple(&input(outline)).unwrap();
    // The outline lands where the alignment says; the picture burns inside the same place.
    near(p.bounds, o.bounds, 1.0);
    assert!(p.bounds.x0 > 30.0, "the picture stayed where it was drawn: {:?}", p.bounds);
}

#[test]
fn rotary_stretches_pictures_like_the_outlines() {
    let (mut pic, mut outline) = picture_and_outline();
    with_vision(&mut pic, |s| s.rotary.enabled = true);
    with_vision(&mut outline, |s| s.rotary.enabled = true);
    let p = build_job_simple(&input(pic)).unwrap();
    let o = build_job_simple(&input(outline)).unwrap();
    assert!(o.bounds.height() > 50.0, "the rotary maps Y to axis units: {:?}", o.bounds);
    near(p.bounds, o.bounds, 1.0);
}

#[test]
fn cylinder_correction_refuses_pictures_it_cannot_bend() {
    let (mut pic, mut outline) = picture_and_outline();
    with_vision(&mut pic, |s| s.cylinder.enabled = true);
    with_vision(&mut outline, |s| s.cylinder.enabled = true);
    assert_eq!(build_job_simple(&input(pic)).unwrap_err(), BuildError::PictureNotMapped);
    assert!(build_job_simple(&input(outline)).is_ok(), "outlines are still bent");
}

/// A 40 mm outline with a 10 mm hole in the middle, cut with a 0.5 mm kerf; returns the hole.
fn hole_doc() -> (Document, darkburn_core::ShapeId) {
    let mut d = Document::new();
    let s = d.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    s.kerf = 0.5;
    sq(&mut d, 0, 10.0, 10.0, 40.0);
    let hole = d.add(darkburn_core::Shape::rect(Rect::new(25.0, 25.0, 35.0, 35.0), 0.0, 0));
    (d, hole)
}

#[test]
fn a_selected_hole_is_still_a_hole_when_its_outline_is_not_selected() {
    let (d, hole) = hole_doc();
    let mut i = input(d);
    i.settings.cut_selected = true;
    i.selection = vec![hole];
    let b = build_job_simple(&i).unwrap();
    // A hole shrinks by the kerf; an outside would grow to (24.5, 24.5) to (35.5, 35.5).
    near(b.bounds, Rect::new(25.5, 25.5, 34.5, 34.5), 0.05);
}

#[test]
fn a_selected_outline_is_offset_alone() {
    // The unselected hole is not cut, and the selected outline still grows as an outside.
    let (d, hole) = hole_doc();
    let outer = d.iter().map(|s| s.id).find(|id| *id != hole).unwrap();
    let mut i = input(d);
    i.settings.cut_selected = true;
    i.selection = vec![outer];
    let b = build_job_simple(&i).unwrap();
    near(b.bounds, Rect::new(9.5, 9.5, 50.5, 50.5), 0.05);
    assert!(b.toolpath.ops.iter().any(|o| matches!(o, darkburn_job::Op::Cut { .. })));
}

/// A line layer 0 and nothing on it yet.
fn line_doc() -> Document {
    let mut d = Document::new();
    let s = d.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    d
}

/// The lowest corner the head goes to, in machine coordinates.
fn machine_min(b: &BuiltJob) -> Point {
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    for p in b.toolpath.ops.iter().filter_map(darkburn_job::Op::target) {
        min = Point::new(min.x.min(p.x), min.y.min(p.y));
    }
    min
}

#[test]
fn selection_origin_is_measured_where_print_and_cut_puts_the_selection() {
    // A square drawn at (10, 10) to (30, 30), moved by the alignment to (50, 35) to (70, 55).
    let mut d = line_doc();
    let id = d.add(darkburn_core::Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
    with_vision(&mut d, |s| s.pnc.fit = Some(fit(Affine::translate((40.0, 25.0)))));
    let mut i = input(d);
    i.selection = vec![id];
    i.settings.start_from = StartMode::UserOrigin;
    i.user_origin = Some(Point::new(0.0, 0.0));
    i.settings.use_selection_origin = true;
    let b = build_job_simple(&i).unwrap();
    // The selected square is the whole job: its corner goes to the user origin. Measured where
    // it was drawn, the job would start 40 and 25 mm off.
    let min = machine_min(&b);
    assert!(min.x.abs() < 1e-6 && min.y.abs() < 1e-6, "the job starts at {min:?}, not at the user origin");
}

#[test]
fn shapes_outside_the_work_area_are_measured_where_print_and_cut_puts_them() {
    // Moved 40 mm right: a square drawn inside the 200 mm bed lands outside it, and one drawn
    // to the left of the bed lands inside.
    let mut d = line_doc();
    d.add(darkburn_core::Shape::rect(Rect::new(150.0, 10.0, 170.0, 30.0), 0.0, 0));
    d.add(darkburn_core::Shape::rect(Rect::new(-30.0, 10.0, -10.0, 30.0), 0.0, 0));
    with_vision(&mut d, |s| s.pnc.fit = Some(fit(Affine::translate((40.0, 0.0)))));
    let mut i = input(d);
    i.settings.discard_out_of_bounds = true;
    let b = build_job_simple(&i).unwrap();
    assert_eq!(b.dropped_shapes, 1);
    assert!(!b.out_of_bounds(), "the square moved off the bed was kept: {:?}", b.bounds);
    near(b.bounds, Rect::new(10.0, 10.0, 30.0, 30.0), 1e-6);
}

#[test]
fn selection_origin_measures_a_group_leaf_by_leaf_under_the_cylinder() {
    // Cylinder radius 40 along X, centre 0: it reaches 62.8 mm round the surface. Of the two
    // squares in the group, the first (Y 30 to 50) is bent and the second (Y 70 to 90) is left
    // alone. Measured as one outline the whole group was left alone, and the job started
    // 2.7 mm below the user origin.
    let mut d = line_doc();
    let a = d.add(darkburn_core::Shape::rect(Rect::new(10.0, 30.0, 30.0, 50.0), 0.0, 0));
    let b = d.add(darkburn_core::Shape::rect(Rect::new(10.0, 70.0, 30.0, 90.0), 0.0, 0));
    let g = darkburn_core::arrange::group(&mut d, &[a, b]).unwrap();
    with_vision(&mut d, |s| s.cylinder.enabled = true);
    let mut i = input(d);
    i.selection = vec![g];
    i.settings.start_from = StartMode::UserOrigin;
    i.user_origin = Some(Point::new(0.0, 0.0));
    i.settings.use_selection_origin = true;
    let with_selection = build_job_simple(&i).unwrap();
    i.settings.use_selection_origin = false;
    let leaf_mapped = build_job_simple(&i).unwrap();
    let (min, want) = (machine_min(&with_selection), machine_min(&leaf_mapped));
    assert!((min.x - want.x).abs() < 1e-6 && (min.y - want.y).abs() < 1e-6, "the job starts at {min:?}, not at {want:?}");
    assert!(min.x.abs() < 1e-6 && min.y.abs() < 1e-6, "the job starts at {min:?}, not at the user origin");
}

#[test]
fn shapes_outside_the_work_area_are_measured_where_the_start_mode_puts_them() {
    // Drawn at (150, 10) to (170, 30) on the 200 mm bed and moved 40 mm right by the alignment:
    // past the bed edge as mapped, but started from the user origin it burns at (0, 0) to
    // (20, 20), well inside.
    let mut d = line_doc();
    d.add(darkburn_core::Shape::rect(Rect::new(150.0, 10.0, 170.0, 30.0), 0.0, 0));
    with_vision(&mut d, |s| s.pnc.fit = Some(fit(Affine::translate((40.0, 0.0)))));
    let mut i = input(d);
    i.settings.discard_out_of_bounds = true;
    i.settings.start_from = StartMode::UserOrigin;
    i.user_origin = Some(Point::new(0.0, 0.0));
    let b = build_job_simple(&i).unwrap();
    assert_eq!(b.dropped_shapes, 0);
    let min = machine_min(&b);
    assert!(min.x.abs() < 1e-6 && min.y.abs() < 1e-6, "the job starts at {min:?}, not at the user origin");
    // A user origin near the far edge puts it off the bed: dropped.
    i.user_origin = Some(Point::new(190.0, 0.0));
    assert_eq!(build_job_simple(&i).unwrap_err(), BuildError::NothingToBurn);
    // From the current position with the head at (0, 0) it fits as well.
    i.settings.start_from = StartMode::CurrentPosition;
    i.head = Some(Point::new(0.0, 0.0));
    assert_eq!(build_job_simple(&i).unwrap().dropped_shapes, 0);
    // With the head position unknown it is measured where it is mapped, as before: dropped.
    i.head = None;
    assert_eq!(build_job_simple(&i).unwrap_err(), BuildError::NothingToBurn);
    // Absolute jobs are measured where they are mapped too.
    i.settings.start_from = StartMode::Absolute;
    assert_eq!(build_job_simple(&i).unwrap_err(), BuildError::NothingToBurn);
}

#[test]
fn a_fill_left_out_for_its_size_does_not_move_the_placement() {
    // A fill 10 metres wide at 0.001 mm a line is left out. Counted in the placement it moved
    // the job 10 m right of the user origin, and the square that does burn was dropped.
    let mut d = line_doc();
    {
        let s = d.layer_mut(1).main_mut();
        s.mode = Mode::Fill;
        s.interval = 0.001;
        s.speed = 100.0;
        s.max_power = 20.0;
    }
    d.add(darkburn_core::Shape::rect(Rect::new(-10000.0, 0.0, 0.0, 10.0), 0.0, 1));
    d.add(darkburn_core::Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
    let mut i = input(d);
    i.settings.discard_out_of_bounds = true;
    i.settings.start_from = StartMode::UserOrigin;
    i.user_origin = Some(Point::new(0.0, 0.0));
    let b = build_job_simple(&i).unwrap();
    assert_eq!(b.dropped_shapes, 0);
    assert!(b.warnings.iter().any(|w| w.contains("scan lines")), "{:?}", b.warnings);
    let min = machine_min(&b);
    assert!(min.x.abs() < 1e-6 && min.y.abs() < 1e-6, "the job starts at {min:?}, not at the user origin");
}
