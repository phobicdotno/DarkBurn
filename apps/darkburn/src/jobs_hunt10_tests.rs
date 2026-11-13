//! Bug hunt 10, jobs: odd jobs built the way the app builds them, sent to the in-memory GRBL
//! emulator, and what the emulator received compared with the job preview and the layers. No
//! port, USB device or network connection is opened.

use std::sync::Arc;
use std::time::{Duration, Instant};

use darkburn_core::kurbo::{BezPath, Point, Rect};
use darkburn_core::layer::Mode;
use darkburn_core::{Document, Shape};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::job_build::BuiltJob;
use crate::job_field_tests::{image_doc, line_doc, sq};
use crate::machine_api::*;
use crate::prefs::Prefs;
use crate::preview::PreviewData;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(60);

#[track_caller]
fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}, log {:?}", app.machine.handle.snapshot().state, app.machine.job_log);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn settle(app: &mut DarkBurn, time: Duration) {
    let end = Instant::now() + time;
    while Instant::now() < end {
        app.machine.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The app with a fast GRBL emulator attached and ready.
fn on_grbl() -> (DarkBurn, darkburn_grbl::EmulatorHandle) {
    let mut a = DarkBurn::new(Prefs::default());
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
    let mut spec = a.machine.active().connect_spec(&[]).unwrap();
    spec.config.sim_time_scale = 5000.0;
    let (adapter, emu) = crate::connections::grbl_simulator(&spec.config);
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    (a, emu)
}

/// One burning move the emulator received: from, to, S, feed in mm/min.
#[derive(Clone, Copy, Debug)]
struct Burn {
    a: Point,
    b: Point,
    s: f64,
    f: f64,
}

/// Read the burning moves out of the G-code lines (G1/G2/G3 with the laser on and S above 0).
fn burns(lines: &[String]) -> Vec<Burn> {
    let (mut pos, mut g, mut s, mut f, mut on, mut rel) = (Point::ZERO, 0u32, 0.0, 0.0, false, false);
    let mut out = Vec::new();
    for raw in lines {
        let line = raw.split(';').next().unwrap_or("").split('(').next().unwrap_or("").trim().to_uppercase();
        let mut to = pos;
        let mut moved = false;
        let mut words = Vec::new();
        let mut cur = String::new();
        for ch in line.chars().filter(|c| !c.is_whitespace()) {
            if ch.is_ascii_alphabetic() && !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        for w in &words {
            let (k, v) = w.split_at(1);
            let Ok(n) = v.parse::<f64>() else { continue };
            match k {
                "G" if n == 0.0 || n == 1.0 || n == 2.0 || n == 3.0 => g = n as u32,
                "G" if n == 90.0 => rel = false,
                "G" if n == 91.0 => rel = true,
                "M" if n == 3.0 || n == 4.0 => on = true,
                "M" if n == 5.0 => on = false,
                "S" => s = n,
                "F" => f = n,
                "X" => {
                    to.x = if rel { pos.x + n } else { n };
                    moved = true;
                }
                "Y" => {
                    to.y = if rel { pos.y + n } else { n };
                    moved = true;
                }
                _ => {}
            }
        }
        if moved {
            if g != 0 && on && s > 0.0 && to != pos {
                out.push(Burn { a: pos, b: to, s, f });
            }
            pos = to;
        }
    }
    out
}

fn len(a: Point, b: Point) -> f64 {
    (b - a).hypot()
}

fn bbox(pts: impl IntoIterator<Item = Point>) -> Option<Rect> {
    let mut r: Option<Rect> = None;
    for p in pts {
        r = Some(match r {
            None => Rect::from_points(p, p),
            Some(r) => r.union_pt(p),
        });
    }
    r
}

/// What the run looked like.
struct Run {
    job: Result<Arc<BuiltJob>, String>,
    preview: Option<PreviewData>,
    started: Result<Value, String>,
    lines: Vec<String>,
    s_max: f64,
}

/// Put `doc` in the app, build it as the app does, then press Start and wait for the end.
fn run_doc(doc: Document, setup: impl FnOnce(&mut DarkBurn)) -> Run {
    let (mut a, emu) = on_grbl();
    a.editor.doc = doc;
    setup(&mut a);
    let job = crate::job_cmds::build_now(&a);
    let preview = job.as_ref().ok().map(|j| PreviewData::new(j.clone(), &a.job.additional, 0));
    let before = emu.with(|e| e.stats().lines.len());
    let started = execute_headless(&mut a, "job.start", &json!({}));
    if started.is_ok() && a.machine.job_log.iter().any(|l| l.starts_with("Job started")) || a.machine.handle.snapshot().state.is_working() {
        wait(&mut a, "the job to end", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished") || l.starts_with("Job stopped")));
    } else {
        settle(&mut a, Duration::from_millis(400));
        if a.machine.job_log.iter().any(|l| l.starts_with("Job started")) {
            wait(&mut a, "the job to end", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished") || l.starts_with("Job stopped")));
        }
    }
    let lines = emu.with(|e| e.stats().lines[before..].to_vec());
    Run { job, preview, started, lines, s_max: a.job.gcode.s_max }
}

/// The burning pieces of the preview: from, to, percent.
fn preview_burns(p: &PreviewData) -> Vec<(Point, Point, f64)> {
    p.segs.iter().filter(|s| !s.rapid && s.power > 0.0 && s.a != s.b).map(|s| (s.a, s.b, s.power as f64)).collect()
}

/// Compare what the emulator burned with what the preview shows: total length, power order,
/// powers in range, feeds and bounds.
fn compare(r: &Run, what: &str) {
    let b = burns(&r.lines);
    let p = r.preview.as_ref().map(preview_burns).unwrap_or_default();
    let gl: f64 = b.iter().map(|x| len(x.a, x.b)).sum();
    let pl: f64 = p.iter().map(|x| len(x.0, x.1)).sum();
    let top = b.iter().map(|x| x.s).fold(0.0, f64::max);
    eprintln!(
        "{what}: started {:?}, {} lines, {} burns {gl:.2} mm (top S{top} of {}), preview {} burns {pl:.2} mm",
        r.started.as_ref().map(|_| ()),
        r.lines.len(),
        b.len(),
        r.s_max,
        p.len()
    );
    assert!((gl - pl).abs() <= 0.01 * pl.max(1.0), "{what}: the machine burned {gl:.3} mm, the preview shows {pl:.3} mm");
    for x in &b {
        assert!(x.s <= r.s_max + 1e-9, "{what}: S{} above the device's S max {}", x.s, r.s_max);
        assert!(x.f > 0.0, "{what}: a burn with no feed: {x:?}");
    }
    // Power order: the run of powers in the order they burn.
    let rle = |v: Vec<f64>| {
        let mut o: Vec<f64> = Vec::new();
        for x in v {
            if o.last().is_none_or(|l| (l - x).abs() > 0.6) {
                o.push(x);
            }
        }
        o
    };
    let gp = rle(b.iter().map(|x| (x.s / r.s_max * 100.0 * 10.0).round() / 10.0).collect());
    let pp = rle(p.iter().map(|x| (x.2 * 10.0).round() / 10.0).collect());
    assert_eq!(gp, pp, "{what}: the powers burn in another order (machine, then preview)");
    // Feeds: every burning feed is a speed of a cut in the job.
    if let Ok(job) = &r.job {
        let speeds: Vec<f64> =
            job.toolpath.ops.iter().filter_map(|o| if let darkburn_job::Op::Cut { speed, .. } = o { Some(*speed) } else { None }).collect();
        for x in &b {
            assert!(speeds.iter().any(|s| (s * 60.0 - x.f).abs() < 0.6), "{what}: feed F{} is no speed of the job {speeds:?}", x.f);
        }
    }
    let gb = bbox(b.iter().flat_map(|x| [x.a, x.b]));
    let pb = bbox(p.iter().flat_map(|x| [x.0, x.1]));
    match (gb, pb) {
        (Some(g), Some(p)) => {
            let d = (g.x0 - p.x0).abs().max((g.y0 - p.y0).abs()).max((g.x1 - p.x1).abs()).max((g.y1 - p.y1).abs());
            assert!(d < 0.01, "{what}: burned within {g:?}, preview shows {p:?}");
        }
        (None, None) => {}
        (g, p) => panic!("{what}: burned within {g:?}, preview shows {p:?}"),
    }
}

fn refused(r: &Run, what: &str) {
    assert!(burns(&r.lines).is_empty(), "{what}: the machine burned although the job should be refused: {:?}", &r.lines[..r.lines.len().min(40)]);
}

fn styled(d: &mut Document, layer: usize, speed: f64, power: f64) {
    let s = d.layer_mut(layer).main_mut();
    s.mode = Mode::Line;
    s.speed = speed;
    s.max_power = power;
}

#[test]
fn jobs_hunt10_a_plain_square_matches_the_preview() {
    let r = run_doc(line_doc(20.0), |_| {});
    assert!(r.started.is_ok(), "{:?}", r.started);
    assert!(!burns(&r.lines).is_empty());
    compare(&r, "plain square");
}

#[test]
fn jobs_hunt10_empty_and_degenerate_jobs_send_no_burn() {
    let r = run_doc(Document::new(), |_| {});
    assert!(r.started.is_err() || r.job.is_err(), "an empty job was started: {:?}", r.started);
    refused(&r, "empty");
    let mut d = Document::new();
    styled(&mut d, 0, 20.0, 50.0);
    let mut p = BezPath::new();
    p.move_to((10.0, 10.0));
    d.add(Shape::path(p, 0));
    let r = run_doc(d, |_| {});
    compare(&r, "one point");
    let mut d = Document::new();
    styled(&mut d, 0, 20.0, 50.0);
    d.add(Shape::rect(Rect::new(10.0, 10.0, 10.0, 10.0), 0.0, 0));
    let r = run_doc(d, |_| {});
    compare(&r, "zero size");
}

#[test]
fn jobs_hunt10_a_shape_outside_the_bed_is_not_burned_unasked() {
    let mut d = line_doc(20.0);
    sq(&mut d, 0, 5000.0, 5000.0, 20.0);
    let r = run_doc(d, |_| {});
    refused(&r, "outside the bed");
}

#[test]
fn jobs_hunt10_odd_settings_keep_to_the_preview() {
    for (what, speed, power, min, passes) in [
        ("speed 0", 0.0, 50.0, 0.0, 1),
        ("power 0", 20.0, 0.0, 0.0, 1),
        ("passes 0", 20.0, 50.0, 0.0, 0),
        ("power 150", 20.0, 150.0, 0.0, 1),
        ("min above max", 20.0, 30.0, 80.0, 1),
    ] {
        let mut d = Document::new();
        styled(&mut d, 0, speed, power);
        d.layer_mut(0).main_mut().min_power = min;
        d.layer_mut(0).main_mut().passes = passes;
        sq(&mut d, 0, 10.0, 10.0, 20.0);
        let r = run_doc(d, |_| {});
        compare(&r, what);
        let top = burns(&r.lines).iter().map(|b| b.s).fold(0.0, f64::max);
        assert!(top <= r.s_max * power.clamp(0.0, 100.0) / 100.0 + 1e-6, "{what}: S{top} is more than the layer's max power allows");
    }
}

#[test]
fn jobs_hunt10_thirty_layers_in_a_mixed_order_burn_in_the_preview_order() {
    let mut d = Document::new();
    for k in 0..30usize {
        let layer = (k * 17) % 30;
        styled(&mut d, layer, 20.0 + layer as f64, 10.0 + 2.0 * layer as f64);
        sq(&mut d, layer, 5.0 + (k % 6) as f64 * 30.0, 5.0 + (k / 6) as f64 * 30.0, 10.0);
    }
    let r = run_doc(d, |_| {});
    compare(&r, "30 layers");
}

#[test]
fn jobs_hunt10_a_thousand_small_shapes() {
    let mut d = Document::new();
    styled(&mut d, 0, 200.0, 40.0);
    for k in 0..1000usize {
        sq(&mut d, 0, 2.0 + (k % 40) as f64 * 4.5, 2.0 + (k / 40) as f64 * 4.5, 2.0);
    }
    let r = run_doc(d, |_| {});
    compare(&r, "1000 shapes");
}

#[test]
fn jobs_hunt10_hidden_and_switched_off_layers() {
    let mut d = line_doc(20.0);
    styled(&mut d, 1, 30.0, 70.0);
    sq(&mut d, 1, 50.0, 50.0, 20.0);
    d.layer_mut(1).show = false;
    let r = run_doc(d.clone(), |_| {});
    compare(&r, "hidden layer");
    d.layer_mut(1).show = true;
    d.layer_mut(1).output = false;
    let r = run_doc(d, |_| {});
    compare(&r, "layer off for output");
    assert!(burns(&r.lines).iter().all(|b| b.a.x < 45.0 && b.b.x < 45.0), "the layer switched off was burned");
}

#[test]
fn jobs_hunt10_a_group_across_layers() {
    let mut d = Document::new();
    styled(&mut d, 0, 20.0, 30.0);
    styled(&mut d, 1, 40.0, 60.0);
    styled(&mut d, 2, 60.0, 90.0);
    let ids: Vec<_> = (0..3).map(|l| d.add(Shape::rect(Rect::new(10.0 + 30.0 * l as f64, 10.0, 30.0 + 30.0 * l as f64, 30.0), 0.0, l))).collect();
    darkburn_core::arrange::group(&mut d, &ids).unwrap();
    let r = run_doc(d, |_| {});
    compare(&r, "group");
}

#[test]
fn jobs_hunt10_an_image_layer() {
    let d = image_doc(|s| s.image_mode = darkburn_core::layer::ImageMode::Threshold, vec![0u8; 16]);
    let r = run_doc(d, |_| {});
    compare(&r, "image");
}
