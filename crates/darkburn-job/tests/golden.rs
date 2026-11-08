//! Golden G-code tests. Regenerate with `DARKBURN_BLESS=1 cargo test -p darkburn-job --test golden`
//! and review the diff by eye before committing.

use std::path::PathBuf;

use darkburn_job::gcode::{self, Flavor, GcodeSettings, MarlinPower};
use darkburn_job::*;
use kurbo::{BezPath, Circle, Rect, Shape};

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
    if std::env::var_os("DARKBURN_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden file {name}; run with DARKBURN_BLESS=1")).replace("\r\n", "\n");
    if expected != actual {
        let first = expected.lines().zip(actual.lines()).position(|(a, b)| a != b);
        panic!("{name} differs (first differing line {first:?}):\n{actual}");
    }
}

fn job(layers: Vec<LayerInput>) -> JobInput {
    JobInput { layers }
}

fn run(input: &JobInput, opts: &JobOptions) -> Toolpath {
    generate(input, opts, &mut Progress::none()).unwrap()
}

fn absolute() -> JobOptions {
    JobOptions {
        origin: Some(OriginSettings { machine: MachineGeometry { width: 200.0, height: 200.0, ..Default::default() }, ..Default::default() }),
        ..Default::default()
    }
}

#[test]
fn square_grbl() {
    let sq = Rect::new(10.0, 10.0, 30.0, 30.0).to_path(0.01);
    let input = job(vec![LayerInput::new(0, "Cut", CutSettings::line(10.0, 60.0)).with_shape(ShapeInput::new(sq))]);
    let tp = run(&input, &absolute());
    check("square_grbl.gcode", &gcode::emit(&tp, &GcodeSettings::grbl()));
}

#[test]
fn circle_tabs_lead_in_grblhal() {
    let c = Circle::new((50.0, 50.0), 10.0).to_path(0.1);
    let mut s = CutSettings::line(8.0, 80.0);
    s.passes = 2;
    s.air = true;
    s.line.tabs = TabSettings { enabled: true, spacing: 30.0, min_count: 2, max_count: 4, size: 1.0, ..Default::default() };
    s.line.lead_in = Some(Lead { length: 2.0, angle_deg: 0.0, style: LeadStyle::Arc });
    let input = job(vec![LayerInput::new(1, "Circle", s).with_shape(ShapeInput::new(c))]);
    let mut opts = JobOptions { tolerance: 0.5, ..absolute() };
    opts.origin.as_mut().unwrap().finish = FinishPosition::MachineOrigin;
    let tp = run(&input, &opts);
    check("circle_grblhal.gcode", &gcode::emit(&tp, &GcodeSettings::new(Flavor::GrblHal)));
}

/// Letter-like shapes: an "O" (outer and inner ring) and an "L" stroke.
fn text_paths() -> Vec<ShapeInput> {
    let mut o = Circle::new((20.0, 20.0), 8.0).to_path(0.1);
    o.extend(Circle::new((20.0, 20.0), 5.0).to_path(0.1));
    let mut l = BezPath::new();
    l.move_to((32.0, 28.0));
    l.line_to((32.0, 12.0));
    l.line_to((42.0, 12.0));
    vec![ShapeInput::new(o), ShapeInput::new(l)]
}

#[test]
fn text_inner_first_marlin_top_left() {
    let mut layer = LayerInput::new(2, "Text", CutSettings::line(20.0, 100.0));
    layer.shapes = text_paths();
    let input = job(vec![layer]);
    let mut opts = JobOptions { tolerance: 0.5, ..absolute() };
    opts.origin.as_mut().unwrap().machine.origin = Corner::TopLeft;
    let tp = run(&input, &opts);
    let mut s = GcodeSettings::new(Flavor::Marlin);
    s.marlin_power = MarlinPower::M3;
    check("text_marlin.gcode", &gcode::emit(&tp, &s));
}

#[test]
fn text_smoothie_current_position() {
    let mut layer = LayerInput::new(2, "Text", CutSettings::line(20.0, 50.0));
    layer.shapes = text_paths();
    let input = job(vec![layer]);
    let opts = JobOptions {
        tolerance: 0.5,
        origin: Some(OriginSettings { start_from: StartFrom::CurrentPosition, anchor: JobAnchor::Center, ..Default::default() }),
        ..Default::default()
    };
    let tp = run(&input, &opts);
    check("text_smoothie_relative.gcode", &gcode::emit(&tp, &GcodeSettings::new(Flavor::Smoothieware)));
}

#[test]
fn fill_with_hole_grbl() {
    let mut p = Rect::new(0.0, 0.0, 10.0, 5.0).to_path(0.01);
    p.extend(Rect::new(4.0, 1.0, 6.0, 4.0).to_path(0.01));
    let mut s = CutSettings::fill(100.0, 30.0, 0.5);
    s.fill.overscan = Overscan::Distance(1.0);
    s.fill.rapid_gap = 1.0;
    let input = job(vec![LayerInput::new(3, "Fill", s).with_shape(ShapeInput::new(p))]);
    let tp = run(&input, &absolute());
    check("fill_grbl.gcode", &gcode::emit(&tp, &GcodeSettings::grbl()));
}

#[test]
fn fill_angled_crosshatch_linuxcnc() {
    let p = Rect::new(0.0, 0.0, 4.0, 4.0).to_path(0.01);
    let mut s = CutSettings::fill(50.0, 40.0, 1.0);
    s.fill.angle_deg = 45.0;
    s.fill.cross_hatch = true;
    s.fill.bidirectional = false;
    let input = job(vec![LayerInput::new(4, "Hatch", s).with_shape(ShapeInput::new(p))]);
    let tp = run(&input, &absolute());
    check("fill_hatch_linuxcnc.gcode", &gcode::emit(&tp, &GcodeSettings::new(Flavor::LinuxCnc)));
}

#[test]
fn image_scanlines_grbl() {
    let lines = vec![
        Scanline { y: 0.0, runs: vec![(0.0, 1.0, 1.0), (1.0, 2.0, 0.5), (2.0, 3.0, 0.0), (3.0, 4.0, 0.25)] },
        Scanline { y: 0.1, runs: vec![(0.0, 4.0, 0.0)] },
        Scanline { y: 0.2, runs: vec![(0.5, 1.5, 1.0)] },
        Scanline { y: 0.3, runs: vec![(0.0, 2.0, 0.5), (2.0, 4.0, 0.5)] },
    ];
    let mut layer = LayerInput::new(5, "Photo", CutSettings::image(80.0, 50.0, 10.0));
    layer.settings[0].image.overscan = Overscan::Percent(2.5);
    layer.images.push(ImageInput::new(lines));
    let tp = run(&job(vec![layer]), &absolute());
    let s = GcodeSettings { overscan_as_g0: false, ..GcodeSettings::grbl() };
    check("image_grbl.gcode", &gcode::emit(&tp, &s));
}

#[test]
fn time_estimate_end_to_end() {
    let mut l = BezPath::new();
    l.move_to((0.0, 0.0));
    l.line_to((100.0, 0.0));
    let input = job(vec![LayerInput::new(0, "Line", CutSettings::line(10.0, 50.0)).with_shape(ShapeInput::new(l))]);
    let tp = run(&input, &JobOptions::default());
    let m = MotionSettings { accel: 50_000.0, ..Default::default() };
    let sim = sim::simulate(&tp, &m);
    assert!((sim.total_time - 10.0).abs() < 0.05, "{}", sim.total_time);
    assert!((sim.cut_distance - 100.0).abs() < 1e-9);
    assert_eq!(sim.rapid_distance, 0.0);
    assert_eq!(sim.per_layer.len(), 1);
    assert!((sim.per_layer[0].time - sim.total_time).abs() < 1e-9);
}

#[test]
fn start_here_and_bounds() {
    let sq = Rect::new(190.0, 10.0, 210.0, 30.0).to_path(0.01);
    let input = job(vec![LayerInput::new(0, "Cut", CutSettings::line(10.0, 60.0)).with_shape(ShapeInput::new(sq))]);
    let tp = run(&input, &absolute());
    let area = Rect::new(0.0, 0.0, 200.0, 200.0);
    let report = check_bounds(&tp, area);
    assert!(!report.is_ok());
    assert!(report.violations.iter().all(|v| v.point.x > 200.0));
    // Start from the third cut.
    let idx = tp.ops.iter().enumerate().filter(|(_, o)| matches!(o, Op::Cut { .. })).nth(2).unwrap().0;
    let part = tp.slice_from(idx);
    assert!(matches!(part.ops[0], Op::LayerStart { .. }));
    let rapid = part.ops.iter().find_map(|o| if let Op::Rapid { to } = o { Some(*to) } else { None }).unwrap();
    assert_eq!(Some(rapid), tp.ops[idx - 1].target());
    let full = sim::simulate(&tp, &MotionSettings::default()).total_time;
    let rest = sim::simulate(&part, &MotionSettings::default()).total_time;
    assert!(rest < full);
}
