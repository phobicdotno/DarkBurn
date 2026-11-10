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

#[test]
fn text_from_a_command_is_held_to_the_ranges_of_the_text_fields() {
    use crate::shape_ops::tests::run;
    use serde_json::json;
    let mut a = app();
    let text = |a: &DarkBurn, id: u64| crate::text_cmds::find_text(&a.editor.doc, ShapeId(id)).unwrap().clone();
    let mut wrong = Vec::new();
    let id = run(&mut a, "text.add", json!({ "text": "Hi", "x": 10, "y": 10, "height": 1e300 })).unwrap()["id"].as_u64().unwrap();
    if text(&a, id).height > 10_000.0 {
        wrong.push(format!("height {:e}", text(&a, id).height));
    }
    run(&mut a, "text.set", json!({ "ids": [id], "height": 1e-300 })).unwrap();
    if text(&a, id).height < 0.1 {
        wrong.push(format!("height {:e}", text(&a, id).height));
    }
    run(&mut a, "text.set", json!({ "ids": [id], "letter_spacing": 1e300, "line_spacing": -1e300 })).unwrap();
    let t = text(&a, id);
    if t.letter_spacing > 1000.0 || t.line_spacing < -100.0 {
        wrong.push(format!("spacing {:e} {:e}", t.letter_spacing, t.line_spacing));
    }
    run(&mut a, "text.set", json!({ "ids": [id], "offset": 9_000_000_000_000_000_000u64 })).unwrap();
    if text(&a, id).offset.abs() > 100_000 {
        wrong.push(format!("offset {}", text(&a, id).offset));
    }
    let b = a.editor.doc.get(ShapeId(id)).unwrap().bbox();
    if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) {
        wrong.push(format!("bbox {b:?}"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------- two ways to the same design

fn gcode(a: &DarkBurn) -> String {
    let job = build_job_simple(&input(a.editor.doc.clone())).unwrap();
    gcode_text(&job, None, &DeviceGcodeSettings::default())
}

/// Every plain shape of the design as (kind, layer, outline rounded to a micrometre), in draw order.
fn leaves(a: &DarkBurn) -> Vec<(String, usize, String)> {
    let round = |p: &darkburn_core::kurbo::BezPath| {
        let mut s = String::new();
        for c in darkburn_geom::path::contours(&darkburn_geom::path::flatten(p, 0.001)) {
            for n in c.nodes() {
                s.push_str(&format!("{:.3},{:.3} ", n.x + 0.0, n.y + 0.0));
            }
            s.push_str(if c.closed { "Z " } else { "| " });
        }
        s.replace("-0.000", "0.000")
    };
    a.editor.doc.iter().flat_map(|s| s.leaves()).map(|l| (l.kind_name().to_string(), l.layer, round(&l.outline()))).collect()
}

fn scene() -> (DarkBurn, Vec<ShapeId>) {
    use crate::shape_ops::tests::run;
    use serde_json::json;
    let mut a = app();
    {
        let s = a.editor.doc.layer_mut(0).main_mut();
        s.mode = Mode::Line;
        s.speed = 20.0;
        s.max_power = 50.0;
        let s = a.editor.doc.layer_mut(1).main_mut();
        s.mode = Mode::Fill;
        s.speed = 100.0;
        s.max_power = 30.0;
        s.interval = 0.5;
    }
    let id = |v: serde_json::Value| ShapeId(v["id"].as_u64().unwrap());
    let r = id(run(&mut a, "shape.add_rect", json!({ "x": 20, "y": 20, "width": 8, "height": 4, "radius": 1, "layer": 0 })).unwrap());
    let e = id(run(&mut a, "shape.add_ellipse", json!({ "x": 30, "y": 20, "width": 6, "height": 4, "layer": 1 })).unwrap());
    let p = id(run(&mut a, "shape.add_polygon", json!({ "cx": 24, "cy": 30, "sides": 5, "radius": 3, "layer": 0 })).unwrap());
    let l = id(run(&mut a, "shape.add_path", json!({ "svg_path": "M20 36 L28 36 C30 36 30 38 28 38", "layer": 0 })).unwrap());
    (a, vec![r, e, p, l])
}

#[test]
fn a_virtual_array_flattened_is_the_array_made_of_real_copies() {
    use crate::shape_ops::tests::{run, select};
    use serde_json::json;
    let arrays = [
        ("arrange.grid_array", json!({ "rows": 2, "cols": 3, "dx": 3, "dy": 2 })),
        (
            "arrange.grid_array",
            json!({ "rows": 3, "cols": 2, "dx": 30, "dy": 25, "centre_to_centre": true, "shift": "alt_rows", "shift_amount": 4, "mirror_alt_rows": true }),
        ),
        (
            "arrange.grid_array",
            json!({ "rows": 2, "cols": 2, "dx": 3, "dy": 2, "shift": "hex", "reverse_x": true, "reverse_y": true, "mirror_alt_cols": true }),
        ),
        ("arrange.circular_array", json!({ "cx": 100, "cy": 100, "count": 5, "start_angle": 30, "sweep": 360, "rotate_items": true })),
        (
            "arrange.circular_array",
            json!({ "cx": 100, "cy": 100, "count": 4, "start_angle": 0, "sweep": 90, "rotate_items": false, "clockwise": true }),
        ),
    ];
    let mut wrong = Vec::new();
    for (id, args) in arrays {
        let (mut real, ids) = scene();
        let start = real.editor.doc.clone();
        select(&mut real, &ids);
        let mut ra = args.clone();
        ra["virtual"] = json!(false);
        run(&mut real, id, ra).unwrap();
        let (mut virt, ids) = scene();
        select(&mut virt, &ids);
        let mut va = args.clone();
        va["virtual"] = json!(true);
        run(&mut virt, id, va).unwrap();
        let (lr, lv) = (leaves(&real), leaves(&virt));
        let (mut sr, mut sv) = (lr.clone(), lv.clone());
        sr.sort();
        sv.sort();
        if sr != sv {
            wrong.push(format!("{id} {args}: the virtual array holds other shapes than the real copies ({} and {})", lv.len(), lr.len()));
        }
        run(&mut virt, "arrange.array_flatten", json!({})).unwrap();
        let mut sf = leaves(&virt);
        sf.sort();
        if sf != sr {
            wrong.push(format!("{id} {args}: flattened, the shapes differ from the real copies"));
        }
        // Undo takes each back to the start, in as many steps as were made.
        run(&mut real, "edit.undo", json!({})).unwrap();
        if real.editor.doc != start {
            wrong.push(format!("{id} {args}: one undo does not take the real copies away"));
        }
        run(&mut virt, "edit.undo", json!({})).unwrap();
        run(&mut virt, "edit.undo", json!({})).unwrap();
        if virt.editor.doc != start {
            wrong.push(format!("{id} {args}: two undos do not take the flattened virtual array away"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn grouping_converting_and_ungrouping_leave_the_job_as_it_was() {
    use crate::shape_ops::tests::{run, select};
    use serde_json::json;
    let (mut a, ids) = scene();
    let start = a.editor.doc.clone();
    let (job0, leaves0) = (gcode(&a), leaves(&a));
    let mut wrong = Vec::new();
    select(&mut a, &ids);
    run(&mut a, "arrange.group", json!({})).unwrap();
    if gcode(&a) != job0 {
        wrong.push("grouped, the job differs".to_string());
    }
    run(&mut a, "arrange.ungroup", json!({})).unwrap();
    if a.editor.doc.iter().map(|s| (&s.kind, s.transform, s.layer)).ne(start.iter().map(|s| (&s.kind, s.transform, s.layer))) {
        wrong.push("group then ungroup does not give the shapes back as they were".to_string());
    }
    if gcode(&a) != job0 {
        wrong.push("ungrouped again, the job differs".to_string());
    }
    run(&mut a, "edit.undo", json!({})).unwrap();
    run(&mut a, "edit.undo", json!({})).unwrap();
    if a.editor.doc != start {
        wrong.push("two undos after group and ungroup do not give the start".to_string());
    }
    select(&mut a, &ids);
    run(&mut a, "edit.convert_path", json!({})).unwrap();
    if leaves(&a).iter().map(|l| (&l.1, &l.2)).ne(leaves0.iter().map(|l| (&l.1, &l.2))) {
        wrong.push(format!("converted to paths, the outlines differ:\n{:?}\n{:?}", leaves(&a), leaves0));
    }
    if gcode(&a) != job0 {
        wrong.push("converted to paths, the job differs".to_string());
    }
    run(&mut a, "edit.undo", json!({})).unwrap();
    if a.editor.doc != start {
        wrong.push("undo after convert to path does not give the start".to_string());
    }
    run(&mut a, "edit.redo", json!({})).unwrap();
    if gcode(&a) != job0 {
        wrong.push("redone, the job differs".to_string());
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------- every shape command on extreme shapes

/// Outlines whose every coordinate is a number, from harmless to absurd.
const NASTY: &[(&str, &str)] = &[
    ("zero line", "M5 5 L5 5"),
    ("zero triangle", "M5 5 L5 5 L5 5 Z"),
    ("collinear closed", "M0 0 L10 0 L20 0 Z"),
    ("out and back", "M0 0 L10 0 L0 0 Z"),
    ("bow tie", "M0 0 L10 10 L10 0 L0 10 Z"),
    ("self touching", "M0 0 L20 0 L20 10 L10 10 L10 20 L20 20 L20 10 L30 10 L30 30 L0 30 Z"),
    ("duplicate nodes", "M0 0 L0 0 L10 0 L10 0 L10 10 L10 10 L0 10 L0 10 Z"),
    ("point cubic", "M3 3 C3 3 3 3 3 3 Z"),
    ("cusp cubic", "M0 0 C10 10 0 10 10 0 Z"),
    ("sliver", "M0 0 L100 0 L100 1e-9 Z"),
    ("tiny square", "M0 0 L1e-7 0 L1e-7 1e-7 L0 1e-7 Z"),
    ("huge square", "M-1e9 -1e9 L1e9 -1e9 L1e9 1e9 L-1e9 1e9 Z"),
    ("far square", "M1e11 1e11 L100000000010 1e11 L100000000010 100000000010 L1e11 100000000010 Z"),
    ("absurd square", "M-1e300 -1e300 L1e300 -1e300 L1e300 1e300 L-1e300 1e300 Z"),
    ("overflowing triangle", "M-1e308 -1e308 L1e308 1e308 L1e308 -1e308 Z"),
    ("overflowing cubic", "M0 0 C1e308 1e308 -1e308 1e308 0 5 Z"),
    ("overflowing open cubic", "M0 0 C1e308 1e308 -1e308 1e308 0 5 L5 5"),
    ("open and closed", "M0 0 L10 0 L10 10 Z M20 0 L30 0 L30 10"),
];

#[test]
fn no_shape_command_crashes_or_hangs_on_an_extreme_outline() {
    use crate::shape_ops::tests::{run, select};
    use serde_json::json;
    let commands: Vec<(&str, serde_json::Value)> = vec![
        ("edit.convert_path", json!({})),
        ("edit.close_path", json!({})),
        ("edit.close_tolerance", json!({ "tolerance": 1.0 })),
        ("edit.auto_join", json!({ "tolerance": 0.1 })),
        ("edit.auto_join", json!({ "tolerance": 1e-300 })),
        ("edit.optimize", json!({})),
        ("edit.optimize", json!({ "tolerance": 1.0 })),
        ("edit.delete_duplicates", json!({})),
        ("edit.delete_overlapping", json!({})),
        ("tools.offset", json!({ "distance": 1.0 })),
        ("tools.offset", json!({ "distance": 1.0, "side": "inward", "join": "sharp" })),
        ("tools.offset", json!({ "distance": 0.5, "side": "both", "open_shapes": true, "outer_only": true })),
        ("tools.weld", json!({})),
        ("tools.union", json!({})),
        ("tools.difference", json!({})),
        ("tools.intersection", json!({})),
        ("tools.cut_shapes", json!({})),
        ("tools.round_corners", json!({ "radius": 2 })),
        ("tools.round_corners", json!({ "radius": -2, "chamfer": true })),
        ("shape.remove_rounded_corners", json!({})),
        ("tools.resize_slots", json!({ "new_width": 3.0 })),
        ("tools.select_smaller", json!({})),
        ("arrange.break_apart", json!({})),
        ("arrange.auto_group", json!({})),
        ("arrange.rubber_band", json!({})),
        ("arrange.nest", json!({})),
        ("arrange.grid_array", json!({ "rows": 2, "cols": 2 })),
        ("arrange.grid_array", json!({ "rows": 2, "cols": 2, "virtual": true })),
        ("arrange.circular_array", json!({ "count": 3 })),
        ("arrange.flip_h", json!({})),
        ("arrange.rotate_by", json!({ "degrees": 33 })),
        ("arrange.dock_left", json!({})),
        ("arrange.group", json!({})),
        ("edit.duplicate", json!({})),
        ("measure.selection", json!({})),
        ("tabs.auto", json!({ "force": true })),
        ("tabs.add", json!({ "x": 5, "y": 0 })),
        ("shape.set_start_point", json!({ "x": 5, "y": 0 })),
        ("shape.reverse_direction", json!({})),
        ("node.edit", json!({ "op": "smooth", "subpath": 0, "node": 0 })),
        ("node.edit", json!({ "op": "insert", "subpath": 0, "seg": 0, "t": 0.5 })),
        ("node.edit", json!({ "op": "delete", "subpath": 0, "node": 0 })),
        ("node.edit", json!({ "op": "to_curve", "subpath": 0, "seg": 0 })),
        ("node.edit", json!({ "op": "break", "subpath": 0, "node": 1 })),
        ("node.edit", json!({ "op": "align", "nodes": [[0, 0], [0, 1], [0, 2]], "align": "left" })),
        ("job", json!({})),
    ];
    let limit = std::time::Duration::from_secs(30);
    let mut failures = Vec::new();
    for (name, svg) in NASTY {
        for (id, args) in &commands {
            let (tx, rx) = std::sync::mpsc::channel();
            let (svg, id, args) = (svg.to_string(), id.to_string(), args.clone());
            let what = format!("{id} {args} on '{name}'");
            if std::env::var_os("HUNT_TRACE").is_some() {
                eprintln!("{what}");
            }
            std::thread::Builder::new()
                .stack_size(32 << 20)
                .spawn(move || {
                    let r = std::panic::catch_unwind(|| {
                        let mut a = app();
                        a.editor.doc.layer_mut(0).main_mut().tabs.enabled = true;
                        let good = add_rect(&mut a, 2.0, 2.0, 6.0, 6.0, 0);
                        let Ok(v) = run(&mut a, "shape.add_path", json!({ "svg_path": svg, "layer": 0 })) else { return String::new() };
                        let bad = ShapeId(v["id"].as_u64().unwrap());
                        // The node and tab commands work on one shape; the others on both.
                        if id.starts_with("node.") || id.starts_with("tabs.") || id.starts_with("shape.") {
                            select(&mut a, &[bad]);
                        } else {
                            select(&mut a, &[good, bad]);
                        }
                        if id == "job" {
                            let _ = build_job_simple(&input(a.editor.doc.clone()));
                            return String::new();
                        }
                        let r = run(&mut a, &id, args);
                        if std::env::var_os("HUNT_TRACE").is_some() {
                            eprintln!(
                                "  -> {}",
                                r.map(|v| v.to_string()).unwrap_or_else(|e| format!("refused: {e}")).chars().take(90).collect::<String>()
                            );
                        }
                        // Whatever it did, the good square is not lost by a command that only tidies.
                        let tidies = [
                            "edit.close_path",
                            "edit.close_tolerance",
                            "edit.auto_join",
                            "edit.optimize",
                            "edit.delete_overlapping",
                            "arrange.break_apart",
                        ];
                        let area: f64 = a
                            .editor
                            .doc
                            .iter()
                            .map(|s| darkburn_geom::path::signed_area(&s.outline()).abs())
                            .filter(|v| v.is_finite() && *v < 1e6)
                            .sum();
                        if tidies.contains(&id.as_str()) && !(35.9..).contains(&area) {
                            return format!("the 6 mm square is gone (area left {area})");
                        }
                        String::new()
                    });
                    let _ = tx.send(match r {
                        Ok(s) => s,
                        Err(e) => format!(
                            "panic: {}",
                            e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                        ),
                    });
                })
                .unwrap();
            match rx.recv_timeout(limit) {
                Ok(s) if s.is_empty() => {}
                Ok(s) => failures.push(format!("{what}: {s}")),
                Err(_) => failures.push(format!("{what}: no answer after {limit:?}")),
            }
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

// ---------------------------------------------------------------- Norwegian

mod norwegian {
    use crate::commands;
    use crate::i18n::cmd_label;
    use crate::ui_harness::Gui;

    fn norwegian_gui() -> Gui {
        let mut g = Gui::new(1920.0, 1080.0, 1.0);
        g.app().prefs.language = "nb".into();
        g.frames(3);
        assert_eq!(crate::i18n::language(), "nb");
        g
    }

    fn hint_of(g: &mut Gui, name: &str) -> String {
        let r = g.rect_of(name).unwrap_or_else(|| panic!("no {name}"));
        g.hover_pos(r.center());
        g.frames(1);
        g.app().hint.clone().unwrap_or_default()
    }

    /// The label of command `id` and its key, as a tooltip shows them.
    fn tip(id: &str) -> String {
        let c = commands::find(id).unwrap();
        let key = crate::keymap::key_for(c);
        if key.is_empty() { cmd_label(c) } else { format!("{} ({key})", cmd_label(c)) }
    }

    #[test]
    fn tooltips_and_the_settings_and_about_windows_are_in_norwegian() {
        let mut g = norwegian_gui();
        let mut wrong = Vec::new();
        // A command's name is translated in the menus: its button says the same.
        assert_eq!(cmd_label(commands::find("tools.rect").unwrap()), "Rektangel");
        let tools: Vec<String> = g.rects("tool:").into_iter().map(|(n, _)| n).collect();
        assert!(tools.len() > 8, "{tools:?}");
        for name in tools {
            let h = hint_of(&mut g, &name);
            let want = tip(name.trim_start_matches("tool:"));
            if h != want {
                wrong.push(format!("{name}: the hint is {h:?}, not {want:?}"));
            }
        }
        // The buttons of the application bar.
        for (name, id) in [("appbar:help", "help.about"), ("appbar:settings", "edit.settings")] {
            let r = g.rect_of(name).unwrap();
            g.hover_pos(r.center());
            g.frames(4);
            let label = cmd_label(commands::find(id).unwrap());
            if !g.visible_texts().iter().any(|t| t.trim() == label) {
                wrong.push(format!("{name}: no tooltip {label:?}"));
            }
        }
        // The windows those two open.
        for id in ["help.about", "edit.settings"] {
            g.run_command(id);
            g.settle();
            let label = cmd_label(commands::find(id).unwrap());
            if !g.window_titles().contains(&label) {
                wrong.push(format!("{id}: the window is called {:?}, not {label:?}", g.window_titles()));
            }
            if g.visible_texts().iter().any(|t| t.trim() == "Close") {
                wrong.push(format!("{id}: its button says Close, not Lukk"));
            }
            g.close_dialogs();
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn tooltips_show_the_key_the_user_gave_a_command_not_the_default_one() {
        let mut g = Gui::standard();
        let mut wrong = Vec::new();
        let mut check = |what: &str, got: String, want: &str| {
            if got != want {
                wrong.push(format!("{what}: {got:?}, not {want:?}"));
            }
        };
        check("the default key", hint_of(&mut g, "tool:tools.rect"), "Rectangle (Ctrl+R)");
        g.exec("hotkeys.set", serde_json::json!({ "command": "tools.rect", "key": "Ctrl+Alt+F7" }));
        g.frames(2);
        check("a changed key", hint_of(&mut g, "tool:tools.rect"), "Rectangle (Ctrl+Alt+F7)");
        g.exec("hotkeys.set", serde_json::json!({ "command": "tools.rect", "key": "" }));
        g.frames(2);
        check("a key taken away", hint_of(&mut g, "tool:tools.rect"), "Rectangle");
        // The ribbon: a key taken away leaves no empty brackets, and a command without a
        // default key that is given one shows it.
        let on_ribbon = |g: &Gui, with_key: bool| {
            crate::ribbon::command_ids()
                .into_iter()
                .find(|id| {
                    commands::find(id).is_some_and(|c| crate::keymap::default_key(c).is_empty() != with_key)
                        && commands::works(id)
                        && g.rect_of(&format!("ribbon:{id}")).is_some()
                })
                .expect("a ribbon button")
        };
        let had = on_ribbon(&g, true);
        g.exec("hotkeys.set", serde_json::json!({ "command": had, "key": "" }));
        g.frames(2);
        check(had, hint_of(&mut g, &format!("ribbon:{had}")), commands::find(had).unwrap().label);
        let had_none = on_ribbon(&g, false);
        g.exec("hotkeys.set", serde_json::json!({ "command": had_none, "key": "Ctrl+Alt+F8" }));
        g.frames(2);
        check(had_none, hint_of(&mut g, &format!("ribbon:{had_none}")), &format!("{} (Ctrl+Alt+F8)", commands::find(had_none).unwrap().label));
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn shortcuts_and_drawing_work_the_same_in_norwegian_as_through_commands() {
        use darkburn_core::Shape;
        use egui::{Key, Modifiers};
        let mut g = norwegian_gui();
        // The keys follow the commands, not their translated names.
        g.key(Key::R, Modifiers::COMMAND);
        g.frames(2);
        assert_eq!(g.app().tool, crate::tools::Tool::Rect, "Ctrl+R picks the rectangle tool");
        g.drag_mm((30.0, 30.0), (90.0, 60.0));
        g.click_id("tool:tools.ellipse");
        g.drag_mm((100.0, 30.0), (140.0, 60.0));
        g.click_id("tool:tools.line");
        g.click_mm(10.0, 80.0);
        g.click_mm(50.0, 80.0);
        g.click_mm(50.0, 100.0);
        g.enter();
        g.frames(2);
        let drawn: Vec<Shape> = g.app().editor.doc.iter().cloned().collect();
        assert_eq!(drawn.len(), 3, "a rectangle, an ellipse and a line");
        // The same three through the commands, which take the design's own coordinates (Y up from
        // the bottom of the 400 mm work area; the canvas of the default device counts from the top).
        let mut a = crate::shape_ops::tests::app();
        let run = crate::shape_ops::tests::run;
        run(&mut a, "shape.add_rect", serde_json::json!({ "x": 30, "y": 340, "width": 60, "height": 30 })).unwrap();
        run(&mut a, "shape.add_ellipse", serde_json::json!({ "x": 100, "y": 340, "width": 40, "height": 30 })).unwrap();
        run(&mut a, "shape.add_line", serde_json::json!({ "points": [[10, 320], [50, 320], [50, 300]] })).unwrap();
        let made: Vec<Shape> = a.editor.doc.iter().cloned().collect();
        let mut wrong = Vec::new();
        for (d, m) in drawn.iter().zip(&made) {
            let (bd, bm) = (d.bbox(), m.bbox());
            let near = (bd.x0 - bm.x0).abs() < 0.01 && (bd.y0 - bm.y0).abs() < 0.01 && (bd.x1 - bm.x1).abs() < 0.01 && (bd.y1 - bm.y1).abs() < 0.01;
            let same_settings = Shape { id: m.id, kind: m.kind.clone(), transform: m.transform, ..d.clone() } == *m;
            if d.kind_name() != m.kind_name() || !near || !same_settings {
                wrong.push(format!("drawn {} {bd:?} {d:?}\nmade {} {bm:?} {m:?}", d.kind_name(), m.kind_name()));
            }
        }
        // Three undo steps either way.
        assert_eq!(g.app().editor.history.len(), a.editor.history.len());
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
