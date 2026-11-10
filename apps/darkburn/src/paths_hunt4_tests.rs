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

// ---------------------------------------------------------------- cut settings: editor and agent

/// Every number of a cut setting with the range the Cut Settings editor holds it to
/// (`cut_settings.rs`): a JSON pointer into the stored setting, lowest, highest.
const CUT_SETTING_RANGES: &[(&str, f64, f64)] = &[
    ("/kerf", -10.0, 10.0),
    ("/perf_cut", 0.01, 1000.0),
    ("/perf_skip", 0.01, 1000.0),
    ("/tabs/spacing", 0.1, 10_000.0),
    ("/tabs/min_count", 0.0, 1000.0),
    ("/tabs/max_count", 0.0, 1000.0),
    ("/tabs/size", 0.0, 100.0),
    ("/tabs/tab_power", 0.0, 100.0),
    ("/start_pause_ms", 0.0, 60_000.0),
    ("/end_pause_ms", 0.0, 60_000.0),
    ("/overcut", 0.0, 100.0),
    ("/pwm_khz", 0.1, 1000.0),
    ("/ppi", 1.0, 10_000.0),
    ("/lead_in/length", 0.0, 100.0),
    ("/lead_in/angle", 0.0, 180.0),
    ("/lead_out/length", 0.0, 100.0),
    ("/lead_out/angle", 0.0, 180.0),
    ("/dot_time_ms", 0.001, 10_000.0),
    ("/dot_spacing", 0.001, 1000.0),
    ("/u_offset", -1000.0, 1000.0),
    ("/z_step", -100.0, 100.0),
    ("/overscan", 0.0, 1000.0),
    ("/angle_increment", -360.0, 360.0),
    ("/ramp_length", 0.0, 100.0),
    ("/dot_width_correction", 0.0, 10.0),
    ("/halftone_cells", 1.0, 1000.0),
    ("/halftone_angle", -360.0, 360.0),
    ("/threshold_ramp", 0.0, 100.0),
    ("/galvo/frequency_khz", 1.0, 4000.0),
    ("/galvo/q_pulse_ns", 1.0, 10_000.0),
    ("/galvo/pause_before_ms", 0.0, 60_000.0),
    ("/galvo/pause_after_ms", 0.0, 60_000.0),
    ("/galvo/global_passes", 1.0, 10_000.0),
    ("/galvo/wobble_size", 0.0, 10.0),
    ("/galvo/wobble_step", 0.0, 10.0),
    ("/galvo/jump_speed", 1.0, 100_000.0),
    ("/galvo/min_jump_delay_us", 0.0, 100_000.0),
    ("/galvo/max_jump_delay_us", 0.0, 100_000.0),
    ("/galvo/jump_distance_limit", 0.0, 1000.0),
    ("/galvo/laser_on_tc_us", -10_000.0, 10_000.0),
    ("/galvo/laser_off_tc_us", -10_000.0, 10_000.0),
    ("/galvo/end_tc_us", 0.0, 100_000.0),
    ("/galvo/polygon_tc_us", 0.0, 100_000.0),
    ("/galvo/drill_dwell_ms", 0.01, 60_000.0),
    ("/galvo/drill_spacing_mm", 0.0, 1000.0),
];

/// The settings object `layer.set_cut_settings` takes, with every number of
/// [`CUT_SETTING_RANGES`] set to `value(lowest, highest, whole number)`.
fn settings_with(current: &serde_json::Value, value: impl Fn(f64, f64, bool) -> serde_json::Value) -> serde_json::Value {
    let mut all = current.clone();
    let mut top = serde_json::Map::new();
    for (pointer, lo, hi) in CUT_SETTING_RANGES {
        let whole = all.pointer(pointer).unwrap_or_else(|| panic!("no cut setting {pointer}")).is_u64();
        *all.pointer_mut(pointer).unwrap() = value(*lo, *hi, whole);
        let key = pointer.trim_start_matches('/').split('/').next().unwrap();
        top.insert(key.to_string(), all[key].clone());
    }
    serde_json::Value::Object(top)
}

#[test]
fn an_agent_cannot_store_a_cut_setting_outside_the_range_the_editor_allows() {
    use darkburn_mcp::Backend;
    let shared = std::rc::Rc::new(std::cell::RefCell::new(DarkBurn::new(crate::prefs::Prefs::default())));
    let mut b = crate::mcp_backend::McpBackend::new(shared.clone(), egui::Context::default(), true);
    let stored = |app: &DarkBurn| serde_json::to_value(app.editor.doc.layer(0).main()).unwrap();
    let start = stored(&shared.borrow());
    let tries: [(&str, Box<dyn Fn(f64, f64, bool) -> serde_json::Value>); 2] = [
        ("far above", Box::new(|_, _, whole| if whole { serde_json::json!(4_000_000_000u64) } else { serde_json::json!(1e12) })),
        ("far below", Box::new(|_, _, whole| if whole { serde_json::json!(0) } else { serde_json::json!(-1e12) })),
    ];
    let mut wrong = Vec::new();
    for (what, value) in &tries {
        let settings = settings_with(&start, value);
        b.execute("layer.set_cut_settings", serde_json::json!({ "layer": 0, "settings": settings }))
            .unwrap_or_else(|e| panic!("{what}: {}", e.message));
        let now = stored(&shared.borrow());
        for (pointer, lo, hi) in CUT_SETTING_RANGES {
            let v = now.pointer(pointer).and_then(serde_json::Value::as_f64).unwrap();
            if !(*lo..=*hi).contains(&v) {
                wrong.push(format!("{pointer} set {what} is stored as {v} (the editor allows {lo} to {hi})"));
            }
        }
    }
    assert!(wrong.is_empty(), "{} settings out of range:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn a_project_file_with_cut_settings_out_of_range_opens_with_them_in_range() {
    let mut a = app();
    let start = serde_json::to_value(a.editor.doc.layer(0).main()).unwrap();
    let settings = settings_with(&start, |_, _, whole| if whole { serde_json::json!(4_000_000_000u64) } else { serde_json::json!(1e12) });
    // Straight into the document, as a file written by something else would hold them.
    let mut raw = start.clone();
    for (k, v) in settings.as_object().unwrap() {
        raw[k] = v.clone();
    }
    *a.editor.doc.layer_mut(0).main_mut() = serde_json::from_value(raw).unwrap();
    add_rect(&mut a, 10.0, 10.0, 20.0, 20.0, 0);
    let dir = std::env::temp_dir().join(format!("darkburn-hunt4-ranges-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ranges.dbrn");
    crate::files::save_to(&mut a, &file).unwrap();
    let mut b = app();
    crate::files::open_path(&mut b, &file).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let now = serde_json::to_value(b.editor.doc.layer(0).main()).unwrap();
    let wrong: Vec<String> = CUT_SETTING_RANGES
        .iter()
        .filter_map(|(pointer, lo, hi)| {
            let v = now.pointer(pointer).and_then(serde_json::Value::as_f64).unwrap();
            (!(*lo..=*hi).contains(&v)).then(|| format!("{pointer} is {v} (the editor allows {lo} to {hi})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} settings out of range:\n{}", wrong.len(), wrong.join("\n"));
}

// ---------------------------------------------------------------- extreme shapes

#[test]
fn copy_along_a_path_whose_length_cannot_be_worked_out_is_refused_not_a_crash() {
    use crate::shape_ops::tests::{run, select};
    use serde_json::json;
    let mut a = app();
    let r = add_rect(&mut a, 0.0, 0.0, 5.0, 5.0, 0);
    // Every coordinate is a number, but the curve is so large that its length is not.
    let p = run(&mut a, "shape.add_path", json!({ "svg_path": "M0 0 C1e308 1e308 -1e308 1e308 0 5 L5 5" })).unwrap();
    let p = ShapeId(p["id"].as_u64().unwrap());
    select(&mut a, &[r, p]);
    let before = a.editor.doc.len();
    for args in [json!({ "count": 3 }), json!({ "spacing": 10.0 }), json!({ "count": 3, "virtual": true })] {
        let _ = run(&mut a, "arrange.copy_along_path", args);
        select(&mut a, &[r, p]);
    }
    for s in a.editor.doc.iter() {
        let b = s.bbox();
        assert!(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite(), "a copy with no real place: {b:?}");
    }
    assert!(a.editor.doc.len() >= before);
}

// ---------------------------------------------------------------- drawing: tools and commands

#[test]
fn the_shape_commands_refuse_what_the_drawing_tools_cannot_draw() {
    use crate::shape_ops::tests::run;
    use serde_json::json;
    let mut a = app();
    // The rectangle, ellipse and polygon tools make nothing from a drag without size.
    let refused = [
        ("shape.add_rect", json!({ "x": 0, "y": 0, "width": 0, "height": 10 })),
        ("shape.add_rect", json!({ "x": 0, "y": 0, "width": 10, "height": 0 })),
        ("shape.add_ellipse", json!({ "x": 0, "y": 0, "width": 0, "height": 0 })),
        ("shape.add_polygon", json!({ "cx": 0, "cy": 0, "sides": 6, "radius": 0 })),
        ("shape.add_polygon", json!({ "cx": 0, "cy": 0, "sides": 6, "radius": -5 })),
        // A layer that does not exist is an error everywhere else.
        ("shape.add_line", json!({ "points": [[0, 0], [10, 0]], "layer": 99 })),
        ("shape.add_line", json!({ "points": [[0, 0], [10, 0]], "layer": -1 })),
    ];
    let mut drawn = Vec::new();
    for (id, args) in refused {
        let r = run(&mut a, id, args.clone());
        if r.is_ok() {
            drawn.push(format!("{id} {args} answered {r:?}"));
        }
    }
    assert!(drawn.is_empty(), "{}", drawn.join("\n"));
    assert_eq!(a.editor.doc.len(), 0, "nothing was drawn");
    assert!(!a.editor.history.can_undo(), "and there is nothing to undo");
    // What the tools can draw still works, a rectangle given from its far corner too.
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": -4, "height": -2 })).unwrap();
    let b = a.editor.doc.iter().next().unwrap().bbox();
    assert!((b.x0 - 6.0).abs() < 1e-9 && (b.y0 - 8.0).abs() < 1e-9 && (b.width() - 4.0).abs() < 1e-9 && (b.height() - 2.0).abs() < 1e-9, "{b:?}");
    run(&mut a, "shape.add_line", json!({ "points": [[0, 0], [10, 0]], "layer": 31 })).unwrap();
}
