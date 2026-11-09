//! Every cut-settings field goes document -> `job_build::build_job` -> toolpath / G-code here.
//! These tests are the proof behind the LAYER, MODE, CUT, LINE, FILL and IMGL checklist items.

use darkburn_core::kurbo::{Affine, Point, Rect};
use darkburn_core::layer::{self, FillGrouping, LaserSelect, Mode};
use darkburn_core::{Document, ImageShape, Shape, ShapeKind};
use darkburn_job::{Corner, Op};

use crate::job_build::*;

pub(crate) fn input(doc: Document) -> BuildInput {
    BuildInput {
        doc,
        selection: Vec::new(),
        settings: JobProjectSettings::default(),
        device: JobDevice::new(200.0, 200.0),
        user_origin: None,
        head: None,
        motion: AdditionalSettings::default(),
        tuning: None,
    }
}

pub(crate) fn build(doc: Document) -> BuiltJob {
    build_job_simple(&input(doc)).unwrap()
}

/// One motion segment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seg {
    pub(crate) from: Point,
    pub(crate) to: Point,
    pub(crate) power: f64,
    pub(crate) kind: char, // R rapid, C cut, O overscan, A arc
}

pub(crate) fn segs(tp: &darkburn_job::Toolpath) -> Vec<Seg> {
    let mut pos = tp.start;
    let mut v = Vec::new();
    for op in &tp.ops {
        let s = match *op {
            Op::Rapid { to } => Some(Seg { from: pos, to, power: 0.0, kind: 'R' }),
            Op::Cut { to, power, .. } => Some(Seg { from: pos, to, power, kind: 'C' }),
            Op::Overscan { to, .. } => Some(Seg { from: pos, to, power: 0.0, kind: 'O' }),
            Op::Arc { to, power, .. } => Some(Seg { from: pos, to, power, kind: 'A' }),
            _ => None,
        };
        if let Some(s) = s {
            pos = s.to;
            v.push(s);
        }
    }
    v
}

fn len(s: &Seg) -> f64 {
    s.from.distance(s.to)
}

fn burn_len(tp: &darkburn_job::Toolpath) -> f64 {
    segs(tp).iter().filter(|s| matches!(s.kind, 'C' | 'A') && s.power > 0.0).map(len).sum()
}

fn off_cut_len(tp: &darkburn_job::Toolpath) -> f64 {
    segs(tp).iter().filter(|s| s.kind == 'C' && s.power == 0.0).map(len).sum()
}

pub(crate) fn count(tp: &darkburn_job::Toolpath, f: impl Fn(&Op) -> bool) -> usize {
    tp.ops.iter().filter(|o| f(o)).count()
}

pub(crate) fn sq(doc: &mut Document, layer: usize, x: f64, y: f64, w: f64) {
    doc.add(Shape::rect(Rect::new(x, y, x + w, y + w), 0.0, layer));
}

pub(crate) fn line_doc(w: f64) -> Document {
    let mut d = Document::new();
    let s = d.layer_mut(0).main_mut();
    s.mode = Mode::Line;
    s.speed = 20.0;
    s.max_power = 50.0;
    sq(&mut d, 0, 10.0, 10.0, w);
    d
}

pub(crate) fn fill_doc(interval: f64, w: f64) -> Document {
    let mut d = Document::new();
    let s = d.layer_mut(0).main_mut();
    s.mode = Mode::Fill;
    s.speed = 100.0;
    s.max_power = 40.0;
    s.min_power = 10.0;
    s.interval = interval;
    s.overscan = 0.0;
    s.bidirectional = true;
    sq(&mut d, 0, 10.0, 10.0, w);
    d
}

fn gcode(b: &BuiltJob) -> String {
    gcode_text(b, None, &DeviceGcodeSettings::default())
}

fn dwells(tp: &darkburn_job::Toolpath) -> Vec<(f64, f64)> {
    tp.ops.iter().filter_map(|o| if let Op::Dwell { ms, power } = o { Some((*ms, *power)) } else { None }).collect()
}

// ----- LAYER / CUT -------------------------------------------------------------------------------

#[test]
fn passes_repeat_the_layer() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().passes = 3;
    let b = build(d);
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::PassStart { .. })), 3);
    assert!((burn_len(&b.toolpath) - 3.0 * 80.0).abs() < 0.01);
}

#[test]
fn speed_and_power_reach_the_gcode() {
    let b = build(line_doc(20.0));
    let g = gcode(&b);
    assert!(g.contains("F1200") && g.contains("S500"), "{g}");
    let dev = DeviceGcodeSettings { s_max: 255.0, ..Default::default() };
    let g = gcode_text(&b, None, &dev);
    assert!(g.contains("S127") || g.contains("S128"), "{g}");
}

#[test]
fn constant_power_switches_m3_and_m4() {
    let mut d = line_doc(20.0);
    let g = gcode(&build(d.clone()));
    assert!(g.contains("M4"), "dynamic power by default: {g}");
    d.layer_mut(0).main_mut().constant_power = true;
    let g = gcode(&build(d));
    assert!(g.contains("M3") && !g.contains("M4"), "{g}");
}

#[test]
fn air_assist_is_switched_by_the_layer() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).air = true;
    let b = build(d);
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::Air { on: true })), 1);
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::Air { on: false })), 1);
    assert!(gcode(&b).contains("M8"));
    let mut d = line_doc(20.0);
    d.layer_mut(0).air = false;
    let b = build(d);
    assert!(!gcode(&b).contains("M8"));
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::Air { .. })), 0);
}

#[test]
fn laser_two_is_selected() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().laser = LaserSelect::Laser2;
    let b = build(d);
    assert!(b.toolpath.ops.iter().any(|o| matches!(o, Op::Laser { index: 2 })));
    assert_eq!(b.toolpath.layers[0].laser, 2);
}

#[test]
fn z_offset_and_step_per_pass() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.passes = 3;
        s.z_offset = 2.0;
        s.z_step = -0.5;
    }
    let mut i = input(d.clone());
    i.device.z_enabled = true;
    let b = build_job_simple(&i).unwrap();
    let zs: Vec<f64> = b.toolpath.ops.iter().filter_map(|o| if let Op::ZMove { z } = o { Some(*z) } else { None }).collect();
    assert_eq!(zs, vec![2.0, 1.5, 1.0]);
    assert!(gcode_text(&b, None, &DeviceGcodeSettings::default()).contains("Z1.5"));
    // A machine without a Z axis writes no Z moves.
    let b = build(d);
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::ZMove { .. })), 0);
}

#[test]
fn sub_layers_run_in_order_and_disabled_ones_are_skipped() {
    let mut d = line_doc(20.0);
    {
        let l = d.layer_mut(0);
        let mut sub = l.settings[0].clone();
        sub.speed = 5.0;
        sub.max_power = 90.0;
        l.settings.push(sub);
        let mut off = l.settings[0].clone();
        off.enabled = false;
        off.max_power = 99.0;
        l.settings.push(off);
    }
    let b = build(d);
    assert_eq!(b.toolpath.layers.len(), 2);
    assert_eq!(b.toolpath.layers[1].max_power, 90.0);
    let s = segs(&b.toolpath);
    let powers: Vec<f64> = s.iter().filter(|s| s.power > 0.0).map(|s| s.power).collect();
    assert!(powers.contains(&50.0) && powers.contains(&90.0) && !powers.contains(&99.0));
    let first90 = s.iter().position(|s| s.power == 90.0).unwrap();
    let last50 = s.iter().rposition(|s| s.power == 50.0).unwrap();
    assert!(last50 < first90);
}

#[test]
fn layer_priority_orders_the_layers() {
    let mut d = Document::new();
    for (l, x) in [(0usize, 10.0), (1, 60.0)] {
        let s = d.layer_mut(l).main_mut();
        s.mode = Mode::Line;
        s.max_power = 10.0 + l as f64;
        sq(&mut d, l, x, 10.0, 10.0);
    }
    let b = build(d.clone());
    assert_eq!(b.toolpath.layers[0].layer_index, 0);
    d.layer_order = vec![1, 0];
    let b = build(d);
    assert_eq!(b.toolpath.layers[0].layer_index, 1, "the layer list order is the burn order");
}

#[test]
fn output_off_layers_are_left_out() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).output = false;
    assert_eq!(build_job_simple(&input(d)).unwrap_err(), BuildError::NothingToBurn);
}

#[test]
fn shape_power_scale_and_priority() {
    let mut d = Document::new();
    d.layer_mut(0).main_mut().max_power = 80.0;
    let far = d.add(Shape::rect(Rect::new(100.0, 100.0, 110.0, 110.0), 0.0, 0));
    let near = d.add(Shape::rect(Rect::new(10.0, 10.0, 20.0, 20.0), 0.0, 0));
    d.get_mut(far).unwrap().priority = -1;
    d.get_mut(near).unwrap().power_scale = 50.0;
    let mut i = input(d);
    i.settings.planner.order_by = vec![darkburn_job::OrderKey::Layer, darkburn_job::OrderKey::Priority];
    let b = build_job_simple(&i).unwrap();
    let s = segs(&b.toolpath);
    let first_burn = s.iter().find(|s| s.power > 0.0).unwrap();
    assert!(first_burn.from.x > 90.0, "the lower priority number is cut first: {first_burn:?}");
    assert!(s.iter().any(|s| s.power == 40.0), "power scale 50 % of 80 %");
    assert!(s.iter().any(|s| s.power == 80.0));
}

// ----- LINE --------------------------------------------------------------------------------------

#[test]
fn kerf_offsets_closed_shapes_by_the_given_distance() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().kerf = 0.5;
    let b = build(d.clone());
    assert!((b.bounds.width() - 21.0).abs() < 0.05);
    d.layer_mut(0).main_mut().kerf = -0.5;
    let b = build(d);
    assert!((b.bounds.width() - 19.0).abs() < 0.05);
}

#[test]
fn perforation_cuts_and_skips() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.perforation = true;
        s.perf_cut = 3.0;
        s.perf_skip = 1.0;
    }
    let b = build(d);
    assert!((burn_len(&b.toolpath) - 60.0).abs() < 0.5, "3 of every 4 mm burn: {}", burn_len(&b.toolpath));
    assert!((off_cut_len(&b.toolpath) - 20.0).abs() < 0.5);
}

#[test]
fn tabs_even_spacing_with_min_and_max_count() {
    let mut d = line_doc(40.0); // perimeter 160
    {
        let t = &mut d.layer_mut(0).main_mut().tabs;
        t.enabled = true;
        t.auto_spacing = true;
        t.spacing = 40.0;
        t.size = 2.0;
        t.min_count = 1;
        t.max_count = 10;
    }
    let b = build(d.clone());
    assert!((off_cut_len(&b.toolpath) - 8.0).abs() < 0.1, "4 tabs of 2 mm: {}", off_cut_len(&b.toolpath));
    d.layer_mut(0).main_mut().tabs.max_count = 2;
    let b = build(d.clone());
    assert!((off_cut_len(&b.toolpath) - 4.0).abs() < 0.1, "capped at 2 tabs");
    {
        let t = &mut d.layer_mut(0).main_mut().tabs;
        t.max_count = 10;
        t.min_count = 6;
    }
    let b = build(d.clone());
    assert!((off_cut_len(&b.toolpath) - 12.0).abs() < 0.1, "at least 6 tabs");
    d.layer_mut(0).main_mut().tabs.enabled = false;
    assert_eq!(off_cut_len(&build(d).toolpath), 0.0);
}

#[test]
fn partly_cut_tabs_use_tab_power() {
    let mut d = line_doc(40.0);
    {
        let t = &mut d.layer_mut(0).main_mut().tabs;
        t.enabled = true;
        t.spacing = 80.0;
        t.size = 2.0;
        t.partial = true;
        t.tab_power = 10.0;
    }
    let b = build(d);
    let tab: f64 = segs(&b.toolpath).iter().filter(|s| s.power == 10.0).map(len).sum();
    assert!((tab - 4.0).abs() < 0.1, "2 tabs of 2 mm at 10 %: {tab}");
}

#[test]
fn tabs_skip_inner_shapes() {
    let mut d = line_doc(60.0);
    sq(&mut d, 0, 30.0, 30.0, 10.0);
    {
        let t = &mut d.layer_mut(0).main_mut().tabs;
        t.enabled = true;
        t.spacing = 20.0;
        t.size = 1.0;
        t.skip_inner = false;
    }
    let with_inner = off_cut_len(&build(d.clone()).toolpath);
    d.layer_mut(0).main_mut().tabs.skip_inner = true;
    let without = off_cut_len(&build(d).toolpath);
    assert!(without > 0.0 && with_inner > without + 1.0, "{with_inner} vs {without}");
}

#[test]
fn manual_tabs_from_the_add_tabs_tool_are_cut_out_of_the_path() {
    let mut d = line_doc(40.0);
    {
        let t = &mut d.layer_mut(0).main_mut().tabs;
        t.enabled = true;
        t.auto_spacing = false;
        t.size = 3.0;
    }
    let id = d.iter().next().unwrap().id;
    d.get_mut(id).unwrap().tabs = vec![[20.0, 0.0]];
    let b = build(d.clone());
    let gap = segs(&b.toolpath).into_iter().find(|s| s.kind == 'C' && s.power == 0.0).expect("a tab gap");
    assert!((off_cut_len(&b.toolpath) - 3.0).abs() < 0.1, "one manual tab: {}", off_cut_len(&b.toolpath));
    assert!((gap.from.x - 50.0).abs() < 0.01 || (gap.to.x - 50.0).abs() < 0.01, "tab sits on the right edge: {gap:?}");
    // With manual-only tabs and none placed, nothing is skipped.
    d.get_mut(id).unwrap().tabs.clear();
    assert_eq!(off_cut_len(&build(d).toolpath), 0.0);
}

#[test]
fn start_and_end_pause_with_cut_through() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.start_pause_ms = 150.0;
        s.end_pause_ms = 75.0;
    }
    let b = build(d.clone());
    assert_eq!(dwells(&b.toolpath), vec![(150.0, 0.0), (75.0, 0.0)]);
    assert!(gcode(&b).contains("G4"));
    {
        let s = d.layer_mut(0).main_mut();
        s.cut_through_start = true;
        s.cut_through_end = true;
        s.cut_through_power = 30.0;
    }
    let b = build(d);
    assert_eq!(dwells(&b.toolpath), vec![(150.0, 30.0), (75.0, 30.0)]);
}

#[test]
fn overcut_runs_past_the_start() {
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().overcut = 3.0;
    let b = build(d);
    assert!((burn_len(&b.toolpath) - 83.0).abs() < 0.05);
}

#[test]
fn lead_in_and_out_line_and_arc() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.lead_in = layer::Lead { enabled: true, length: 2.0, angle: 90.0, style: layer::LeadStyle::Line };
        s.lead_out = layer::Lead { enabled: true, length: 2.0, angle: 90.0, style: layer::LeadStyle::Line };
    }
    let b = build(d.clone());
    assert!((burn_len(&b.toolpath) - 84.0).abs() < 0.05, "80 + 2 + 2: {}", burn_len(&b.toolpath));
    {
        let s = d.layer_mut(0).main_mut();
        s.lead_in.style = layer::LeadStyle::Arc;
        s.lead_out.style = layer::LeadStyle::Arc;
    }
    let b = build(d.clone());
    assert_eq!(count(&b.toolpath, |o| matches!(o, Op::Arc { .. })), 2);
    let g = gcode(&b);
    assert!(g.contains("G2") || g.contains("G3"));
    d.layer_mut(0).main_mut().lead_in.enabled = false;
    d.layer_mut(0).main_mut().lead_out.enabled = false;
    assert_eq!(count(&build(d).toolpath, |o| matches!(o, Op::Arc { .. })), 0);
}

#[test]
fn dot_mode_fires_dwells_along_the_path() {
    let mut d = line_doc(20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.dot_mode = true;
        s.dot_time_ms = 4.0;
        s.dot_spacing = 2.0;
    }
    let b = build(d);
    let dots: Vec<f64> = dwells(&b.toolpath).into_iter().filter(|d| d.1 > 0.0).map(|d| d.0).collect();
    assert_eq!(dots.len(), 40, "80 mm / 2 mm");
    assert!(dots.iter().all(|m| *m == 4.0));
    assert_eq!(burn_len(&b.toolpath), 0.0, "dots, not lines");
}

// ----- FILL --------------------------------------------------------------------------------------

#[test]
fn fill_interval_and_lines_per_inch() {
    let rows = |i: f64| segs(&build(fill_doc(i, 10.0)).toolpath).iter().filter(|s| s.kind == 'C' && s.power > 0.0).count();
    assert_eq!(rows(1.0), 10);
    assert_eq!(rows(0.5), 20);
    let s = layer::CutSetting { interval: 25.4 / 254.0, ..Default::default() };
    assert!((s.dpi() - 254.0).abs() < 1e-9);
}

fn dirs(b: &BuiltJob) -> Vec<bool> {
    segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).map(|s| s.to.x > s.from.x).collect()
}

#[test]
fn fill_bidirectional_alternates_and_unidirectional_does_not() {
    let rows = |bi: bool| {
        let mut d = fill_doc(1.0, 10.0);
        d.layer_mut(0).main_mut().bidirectional = bi;
        dirs(&build(d))
    };
    let bi = rows(true);
    assert!(bi.windows(2).all(|w| w[0] != w[1]), "{bi:?}");
    let uni = rows(false);
    assert!(uni.iter().all(|f| *f == uni[0]), "{uni:?}");
}

#[test]
fn fill_overscan_as_distance_and_percent() {
    let mut d = fill_doc(1.0, 10.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.overscan = 2.0;
        s.overscan_percent = false;
    }
    let os: Vec<Seg> = segs(&build(d.clone()).toolpath).into_iter().filter(|s| s.kind == 'O').collect();
    assert!(!os.is_empty() && os.iter().all(|s| (len(s) - 2.0).abs() < 1e-6), "{os:?}");
    {
        let s = d.layer_mut(0).main_mut();
        s.overscan = 5.0; // percent of 100 mm/s
        s.overscan_percent = true;
    }
    let os: Vec<Seg> = segs(&build(d.clone()).toolpath).into_iter().filter(|s| s.kind == 'O').collect();
    assert!(!os.is_empty() && os.iter().all(|s| (len(s) - 5.0).abs() < 1e-6), "5 % of 100 mm/s is 5 mm");
    // The distance readout in the editor shows the same number the job uses.
    assert!((d.layer(0).main().overscan_mm() - 5.0).abs() < 1e-9);
    d.layer_mut(0).main_mut().overscan = 0.0;
    assert_eq!(segs(&build(d).toolpath).iter().filter(|s| s.kind == 'O').count(), 0);
}

#[test]
fn fill_scan_angle_and_angle_increment_per_pass() {
    let mut d = fill_doc(1.0, 20.0);
    d.layer_mut(0).main_mut().scan_angle = 90.0;
    let b = build(d.clone());
    let s: Vec<Seg> = segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).collect();
    assert!(s.iter().all(|s| (s.from.x - s.to.x).abs() < 1e-6 && (s.from.y - s.to.y).abs() > 1.0), "vertical scan lines");
    {
        let m = d.layer_mut(0).main_mut();
        m.scan_angle = 0.0;
        m.angle_increment = 90.0;
        m.passes = 2;
    }
    let b = build(d);
    let s: Vec<Seg> = segs(&b.toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).collect();
    let horizontal = s.iter().filter(|s| (s.from.y - s.to.y).abs() < 1e-6).count();
    let vertical = s.iter().filter(|s| (s.from.x - s.to.x).abs() < 1e-6).count();
    assert!(horizontal >= 15 && vertical >= 15, "pass 1 horizontal, pass 2 turned 90 degrees: {horizontal} {vertical}");
}

#[test]
fn fill_cross_hatch_scans_twice() {
    let mut d = fill_doc(1.0, 10.0);
    let single = burn_len(&build(d.clone()).toolpath);
    d.layer_mut(0).main_mut().cross_hatch = true;
    let both = burn_len(&build(d).toolpath);
    assert!((both - 2.0 * single).abs() < 0.5, "{single} {both}");
}

#[test]
fn fill_ramp_length() {
    let mut d = fill_doc(2.0, 20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.ramp_length = 4.0;
        s.ramp_outer = true;
    }
    let b = build(d.clone());
    let powers: Vec<f64> = segs(&b.toolpath).iter().filter(|s| s.power > 0.0).map(|s| s.power).collect();
    assert!(powers.iter().any(|p| *p < 40.0 && *p >= 10.0), "ramped powers between min and max: {powers:?}");
    assert!(powers.iter().any(|p| (*p - 40.0).abs() < 1e-9));
    d.layer_mut(0).main_mut().ramp_length = 0.0;
    let powers: Vec<f64> = segs(&build(d).toolpath).iter().filter(|s| s.power > 0.0).map(|s| s.power).collect();
    assert!(powers.iter().all(|p| (*p - 40.0).abs() < 1e-9));
}

#[test]
fn fill_ramp_outer_off_leaves_the_edge_unramped() {
    // A square with a square hole: each middle row has two runs.
    let mut d = fill_doc(2.0, 30.0);
    sq(&mut d, 0, 15.0, 15.0, 20.0);
    {
        let s = d.layer_mut(0).main_mut();
        s.ramp_length = 3.0;
        s.ramp_outer = false;
    }
    let ramped = |d: &Document| segs(&build(d.clone()).toolpath).iter().filter(|s| s.power > 0.0 && s.power < 40.0).count();
    let off = ramped(&d);
    d.layer_mut(0).main_mut().ramp_outer = true;
    let on = ramped(&d);
    assert!(on > off, "{on} {off}");
}

#[test]
fn fill_flood_fill_changes_the_row_order() {
    let mut d = fill_doc(1.0, 10.0);
    sq(&mut d, 0, 40.0, 10.0, 10.0);
    d.layer_mut(0).main_mut().bidirectional = false;
    let order = |flood: bool| {
        let mut d = d.clone();
        d.layer_mut(0).main_mut().flood_fill = flood;
        segs(&build(d).toolpath).into_iter().filter(|s| s.kind == 'C' && s.power > 0.0).map(|s| (s.from.x < 30.0, s.from.y)).collect::<Vec<_>>()
    };
    let normal = order(false);
    let flood = order(true);
    assert_eq!(normal.len(), flood.len());
    let switches = |v: &[(bool, f64)]| v.windows(2).filter(|w| w[0].0 != w[1].0).count();
    assert!(switches(&flood) < switches(&normal), "flood fill finishes a region before the next: {} {}", switches(&flood), switches(&normal));
}

#[test]
fn fill_grouping_all_at_once_versus_individually() {
    // Two overlapping squares: scanned together the overlap is a hole (even-odd), one by one it
    // is burnt twice.
    let mut d = fill_doc(1.0, 10.0);
    sq(&mut d, 0, 15.0, 10.0, 10.0);
    let burn = |g: FillGrouping| {
        let mut d = d.clone();
        d.layer_mut(0).main_mut().fill_grouping = g;
        burn_len(&build(d).toolpath)
    };
    let all = burn(FillGrouping::AllAtOnce);
    let one = burn(FillGrouping::Individually);
    assert!(one > all + 20.0, "{all} {one}");
}

#[test]
fn fill_grouping_groups_together_follows_groups() {
    let mut d = fill_doc(1.0, 10.0);
    sq(&mut d, 0, 15.0, 10.0, 10.0);
    sq(&mut d, 0, 50.0, 10.0, 10.0);
    let ids: Vec<_> = d.iter().map(|s| s.id).collect();
    darkburn_core::arrange::group(&mut d, &ids[..2]);
    let burn = |g: FillGrouping| {
        let mut d = d.clone();
        d.layer_mut(0).main_mut().fill_grouping = g;
        burn_len(&build(d).toolpath)
    };
    let together = burn(FillGrouping::GroupsTogether);
    let indiv = burn(FillGrouping::Individually);
    let all = burn(FillGrouping::AllAtOnce);
    assert!((together - all).abs() < 1.0, "the grouped pair scans as one, like all at once here: {together} {all}");
    assert!(indiv > together + 20.0, "{together} {indiv}");
}

#[test]
fn offset_fill_makes_rings_and_respects_power_scale() {
    let mut d = fill_doc(1.0, 10.0);
    d.layer_mut(0).main_mut().mode = Mode::OffsetFill;
    let id = d.iter().next().unwrap().id;
    d.get_mut(id).unwrap().power_scale = 50.0;
    let b = build(d);
    let s = segs(&b.toolpath);
    assert_eq!(s.iter().filter(|s| s.kind == 'R').count(), 1, "one continuous spiral");
    assert!(s.iter().filter(|s| s.kind == 'C').all(|s| (s.power - 20.0).abs() < 1e-9), "50 % of 40 %");
    assert!(burn_len(&b.toolpath) > 100.0);
}

#[test]
fn fill_and_line_runs_both() {
    let mut d = fill_doc(1.0, 10.0);
    d.layer_mut(0).main_mut().mode = Mode::FillLine;
    let b = build(d);
    assert!((burn_len(&b.toolpath) - (100.0 + 40.0)).abs() < 1.0, "{}", burn_len(&b.toolpath));
}

// ----- IMGL --------------------------------------------------------------------------------------

pub(crate) fn image_doc(setting: impl Fn(&mut layer::CutSetting), px: Vec<u8>) -> Document {
    let mut d = Document::new();
    {
        let s = d.layer_mut(3).main_mut();
        s.mode = Mode::Image;
        s.interval = 0.5;
        s.speed = 200.0;
        s.max_power = 60.0;
        s.min_power = 20.0;
        s.overscan = 0.0;
        setting(s);
    }
    // 4 x 4 grey PNG placed as 10 x 10 mm at (5, 5) to (15, 15).
    let img = image::GrayImage::from_raw(4, 4, px).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let key = d.add_image(darkburn_core::ImageData { format: "png".into(), bytes: std::sync::Arc::from(png), px_width: 4, px_height: 4 });
    d.add(Shape::new(
        ShapeKind::Image(ImageShape { image: key, width: 10.0, height: 10.0, ..Default::default() }),
        Affine::translate((10.0, 10.0)),
        3,
    ));
    d
}

#[test]
fn image_modes_all_burn() {
    use layer::ImageMode as M;
    // A coarse chequerboard with grey tones has edges for the sketch mode and tones for the others.
    let grad: Vec<u8> = (0..16).map(|i| if (i / 4 + i % 4) % 2 == 0 { 20 } else { 200 }).collect();
    for m in M::ALL {
        let r = build_job_simple(&input(image_doc(|s| s.image_mode = m, grad.clone())));
        let b = r.unwrap_or_else(|e| panic!("{m:?}: {e}"));
        assert!(burn_len(&b.toolpath) > 5.0, "{m:?} produced nothing");
    }
}

#[test]
fn image_negative_inverts_the_burn() {
    // Left half black, right half white.
    let half: Vec<u8> = (0..16).map(|i| if i % 4 < 2 { 0 } else { 255 }).collect();
    let xs = |neg: bool| {
        let b = build(image_doc(
            |s| {
                s.image_mode = layer::ImageMode::Threshold;
                s.negative = neg;
            },
            half.clone(),
        ));
        let burning: Vec<Seg> = segs(&b.toolpath).into_iter().filter(|s| s.power > 0.0).collect();
        (burning.iter().map(|s| s.from.x.min(s.to.x)).fold(f64::MAX, f64::min), burning.iter().map(|s| s.from.x.max(s.to.x)).fold(f64::MIN, f64::max))
    };
    let (a0, a1) = xs(false);
    assert!(a0 < 7.0 && a1 < 11.0, "positive burns the dark left half: {a0} {a1}");
    let (b0, b1) = xs(true);
    assert!(b0 > 9.0 && b1 > 13.0, "negative burns the light right half: {b0} {b1}");
}

#[test]
fn image_grayscale_min_and_max_power() {
    let grad: Vec<u8> = (0..16).map(|i| (i % 4 * 80) as u8).collect();
    let b = build(image_doc(|s| s.image_mode = layer::ImageMode::Grayscale, grad));
    let p: Vec<f64> = segs(&b.toolpath).iter().filter(|s| s.power > 0.0).map(|s| s.power).collect();
    let (lo, hi) = (p.iter().cloned().fold(f64::MAX, f64::min), p.iter().cloned().fold(0.0, f64::max));
    assert!(lo >= 20.0 - 1e-6 && hi <= 60.0 + 1e-6 && hi - lo > 10.0, "grays map between min 20 and max 60: {lo} {hi}");
}

#[test]
fn image_overscan_bidirectional_and_angle() {
    let black = vec![0u8; 16];
    let b = build(image_doc(
        |s| {
            s.overscan = 3.0;
            s.overscan_percent = false;
            s.image_mode = layer::ImageMode::Threshold;
        },
        black.clone(),
    ));
    let os: Vec<Seg> = segs(&b.toolpath).into_iter().filter(|s| s.kind == 'O').collect();
    assert!(!os.is_empty() && os.iter().all(|s| (len(s) - 3.0).abs() < 1e-6));
    let d = |bi: bool| {
        dirs(&build(image_doc(
            |s| {
                s.bidirectional = bi;
                s.image_mode = layer::ImageMode::Threshold;
            },
            black.clone(),
        )))
    };
    let bi = d(true);
    assert!(bi.windows(2).all(|w| w[0] != w[1]));
    let uni = d(false);
    assert!(uni.iter().all(|f| *f == uni[0]));
    let b = build(image_doc(
        |s| {
            s.scan_angle = 90.0;
            s.image_mode = layer::ImageMode::Threshold;
        },
        black,
    ));
    assert!(segs(&b.toolpath).iter().filter(|s| s.kind == 'C' && s.power > 0.0).all(|s| (s.from.x - s.to.x).abs() < 1e-6));
}

// ----- Device ------------------------------------------------------------------------------------

#[test]
fn swap_xy_and_negative_coordinates_change_the_output() {
    let mut d = Document::new();
    d.layer_mut(0).main_mut().max_power = 10.0;
    d.add(Shape::rect(Rect::new(10.0, 20.0, 40.0, 30.0), 0.0, 0));
    let bounds = |f: &dyn Fn(&mut JobDevice)| {
        let mut i = input(d.clone());
        f(&mut i.device);
        let b = build_job_simple(&i).unwrap();
        let pts: Vec<Point> = b.toolpath.ops.iter().filter_map(Op::target).collect();
        pts.iter()
            .fold(Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN), |r, p| Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y)))
    };
    let plain = bounds(&|_| {});
    assert!((plain.width() - 30.0).abs() < 1e-6 && (plain.x0 - 10.0).abs() < 1e-6);
    let swapped = bounds(&|dev| dev.swap_xy = true);
    assert!((swapped.width() - 10.0).abs() < 1e-6 && (swapped.height() - 30.0).abs() < 1e-6, "{swapped:?}");
    let neg = bounds(&|dev| dev.negative_coords = true);
    assert!(neg.x1 <= 1e-6 && neg.y1 <= 1e-6, "negative-coordinate machines burn below zero: {neg:?}");
    let corner = bounds(&|dev| dev.origin = Corner::TopRight);
    assert!((corner.x1 - (200.0 - 10.0)).abs() < 1e-6 && (corner.y1 - (200.0 - 20.0)).abs() < 1e-6, "{corner:?}");
}

#[test]
fn shape_start_point_and_direction_overrides_survive_the_optimiser() {
    use darkburn_core::CutDirection;
    let mut d = line_doc(20.0);
    let id = d.iter().next().unwrap().id;
    let first_rapid = |d: &Document| {
        let b = build(d.clone());
        b.toolpath.ops.iter().find_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).unwrap()
    };
    // The square is (10,10)-(30,30); a start point on the top edge, in document coordinates.
    let doc_pt = Point::new(20.0, 30.0);
    let local = d.get(id).unwrap().transform.inverse() * doc_pt;
    d.get_mut(id).unwrap().start_point = Some([local.x, local.y]);
    let p = first_rapid(&d);
    assert!(p.distance(doc_pt) < 1e-6, "the cut starts at the forced start point: {p:?}");
    // Clockwise or counter-clockwise from there.
    let heading = |d: &Document| {
        let b = build(d.clone());
        let s = segs(&b.toolpath);
        s.iter().find(|s| s.kind == 'C' && s.power > 0.0).map(|s| s.to - s.from).unwrap()
    };
    d.get_mut(id).unwrap().direction = CutDirection::Clockwise;
    assert!(heading(&d).x > 0.0, "clockwise along the top edge heads right");
    d.get_mut(id).unwrap().direction = CutDirection::CounterClockwise;
    assert!(heading(&d).x < 0.0, "counter-clockwise along the top edge heads left");
}

#[test]
fn set_props_command_sets_direction_and_start_point() {
    let mut app = crate::shell::DarkBurn::new(crate::prefs::Prefs::default());
    let id = app.editor.doc.add(Shape::rect(Rect::new(10.0, 10.0, 30.0, 30.0), 0.0, 0));
    app.editor.selection.set([id]);
    let r = crate::api::execute_headless(&mut app, "shape.set_props", &serde_json::json!({ "direction": "clockwise", "start_point": [20.0, 30.0] }));
    assert!(r.is_ok(), "{r:?}");
    let s = app.editor.doc.get(id).unwrap();
    assert_eq!(s.direction, darkburn_core::CutDirection::Clockwise);
    let sp = s.start_point.unwrap();
    let doc = s.transform * Point::new(sp[0], sp[1]);
    assert!(doc.distance(Point::new(20.0, 30.0)) < 1e-9);
    crate::api::execute_headless(&mut app, "shape.set_props", &serde_json::json!({ "start_point": null, "direction": "auto" })).unwrap();
    let s = app.editor.doc.get(id).unwrap();
    assert!(s.start_point.is_none() && s.direction == darkburn_core::CutDirection::Auto);
    assert!(crate::api::execute_headless(&mut app, "shape.set_props", &serde_json::json!({ "direction": "sideways" })).is_err());
}

#[test]
fn speed_units_convert_between_mm_s_and_mm_min() {
    let per_min = crate::options::Options { speed_per_min: true, ..Default::default() };
    assert_eq!(per_min.speed_value(10.0), 600.0);
    assert_eq!(per_min.speed_to_mm_s(600.0), 10.0);
    let per_s = crate::options::Options::default();
    assert_eq!(per_s.speed_value(10.0), 10.0);
    // The job always carries mm/s: a layer typed as 600 mm/min writes F600.
    let mut d = line_doc(20.0);
    d.layer_mut(0).main_mut().speed = per_min.speed_to_mm_s(600.0);
    assert!(gcode(&build(d)).contains("F600"));
}
