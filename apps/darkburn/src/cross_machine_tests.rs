//! One small design run on every machine that has an emulator, start to finish through the app:
//! every GRBL firmware, Smoothieware, Marlin, Snapmaker, a custom G-code machine, a Ruida
//! controller and a galvo board. What each emulator recorded as burned (where the laser was on,
//! and how strong) is brought back to design coordinates and compared: every machine must burn
//! the same geometry at the same place and scale, honour its origin corner, and end with the
//! laser off.
//!
//! The design: a 30 x 20 mm rectangle cut as a line with a kerf offset, two tabs and two passes;
//! a 20 x 10 mm rectangle filled at 1 mm; an 8 x 8 mm picture, a checkerboard of 2 mm squares.
//!
//! LinuxCNC has no emulator (it is a file-only device) and is not in here.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use darkburn_core::kurbo::{Affine, Point, Rect};
use darkburn_core::layer::{ImageMode, Mode};
use darkburn_core::{ImageShape, Shape, ShapeKind};
use darkburn_galvo as galvo;
use darkburn_job::Corner;
use darkburn_ruida as ruida;
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::connections::SimFirmware;
use crate::devices::DeviceType;
use crate::gcode_machines::SimGcode;
use crate::machine_api::*;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(120);

/// A stretch the laser was on for: design coordinates (mm), power in percent of full power.
#[derive(Clone, Copy, Debug)]
struct Burn {
    a: Point,
    b: Point,
    power: f64,
}

/// What a machine burned of one part of the design.
#[derive(Clone, Copy, Debug)]
struct Part {
    length: f64,
    bounds: Rect,
    power: (f64, f64),
}

/// The line rectangle, the filled rectangle and the picture: they lie apart, so a burn belongs
/// to the part whose corner of the design it is in.
fn parts(burns: &[Burn]) -> [Part; 3] {
    let empty = Part { length: 0.0, bounds: Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN), power: (f64::MAX, f64::MIN) };
    let mut out = [empty; 3];
    for b in burns {
        let mid = b.a.midpoint(b.b);
        let i = if mid.y > 50.0 {
            2
        } else if mid.x > 60.0 {
            1
        } else {
            0
        };
        let p = &mut out[i];
        p.length += b.a.distance(b.b);
        p.bounds = p.bounds.union_pt(b.a).union_pt(b.b);
        p.power = (p.power.0.min(b.power), p.power.1.max(b.power));
    }
    out
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The app with the design in it and a device of `kind` selected, its origin at `corner`.
fn app_with_design(kind: DeviceType, corner: Corner, firmware: Option<SimFirmware>) -> DarkBurn {
    let mut a = DarkBurn::new(Prefs::default());
    let id = run(&mut a, "device.add", json!({ "type": format!("{kind:?}") })).unwrap()["id"].as_u64().unwrap();
    {
        let d = a.machine.devices.get_mut(id).unwrap();
        d.origin = corner;
        if let Some(fw) = firmware {
            d.settings.conn.sim_firmware = fw;
        }
    }
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    crate::machine_ui::sync_job_state(&mut a);

    let doc = &mut a.editor.doc;
    {
        let s = doc.layer_mut(0).main_mut();
        s.mode = Mode::Line;
        s.speed = 100.0;
        s.max_power = 60.0;
        s.min_power = 60.0;
        s.passes = 2;
        s.kerf = 0.2;
        s.tabs.enabled = true;
        s.tabs.auto_spacing = true;
        s.tabs.spacing = 50.0;
        s.tabs.min_count = 2;
        s.tabs.max_count = 2;
        s.tabs.size = 2.0;
    }
    {
        let s = doc.layer_mut(1).main_mut();
        s.mode = Mode::Fill;
        s.speed = 100.0;
        s.max_power = 40.0;
        s.min_power = 40.0;
        s.interval = 1.0;
        s.overscan = 0.0;
        s.bidirectional = true;
    }
    {
        let s = doc.layer_mut(2).main_mut();
        s.mode = Mode::Image;
        s.image_mode = ImageMode::Threshold;
        s.speed = 100.0;
        s.max_power = 50.0;
        s.min_power = 50.0;
        s.interval = 0.5;
        s.overscan = 0.0;
        s.bidirectional = true;
    }
    doc.add(Shape::rect(Rect::new(20.0, 20.0, 50.0, 40.0), 0.0, 0));
    doc.add(Shape::rect(Rect::new(70.0, 20.0, 90.0, 30.0), 0.0, 1));
    // A checkerboard of four by four black and white squares of 2 mm, 8 x 8 mm at (20, 60) to
    // (28, 68). It has one pixel for every dot of the 0.5 mm scan, so nothing is resampled.
    let px: Vec<u8> = (0..256).map(|i| if (i % 16 / 4 + i / 16 / 4) % 2 == 0 { 0 } else { 255 }).collect();
    let img = image::GrayImage::from_raw(16, 16, px).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let key = doc.add_image(darkburn_core::ImageData { format: "png".into(), bytes: Arc::from(png), px_width: 16, px_height: 16 });
    doc.add(Shape::new(
        ShapeKind::Image(ImageShape { image: key, width: 8.0, height: 8.0, ..Default::default() }),
        Affine::translate((24.0, 64.0)),
        2,
    ));
    a
}

/// Machine coordinates back to design coordinates for a bed of `w` x `h` with its zero at
/// `corner` (written out here, not taken from the job builder).
fn to_design(p: Point, corner: Corner, w: f64, h: f64) -> Point {
    match corner {
        Corner::BottomLeft => p,
        Corner::TopLeft => Point::new(p.x, h - p.y),
        Corner::TopRight => Point::new(w - p.x, h - p.y),
        Corner::BottomRight => Point::new(w - p.x, p.y),
    }
}

fn start_and_finish(a: &mut DarkBurn, what: &str) {
    wait(a, &format!("{what}: ready"), |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(a, "job.start", Value::Null).unwrap_or_else(|e| panic!("{what}: start: {e}"));
    wait(a, &format!("{what}: finish"), |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    wait(a, &format!("{what}: ready again"), |a| a.machine.handle.snapshot().state == LaserState::Ready);
    assert!(!a.machine.handle.snapshot().fire_on, "{what}: the laser is off after the job");
    assert!(!a.machine.job_log.iter().any(|l| l.starts_with("Job stopped")), "{what}: {:?}", a.machine.job_log);
}

fn spec_of(a: &DarkBurn) -> ConnectSpec {
    let mut spec = a.machine.active().connect_spec(&[]).unwrap();
    spec.config.sim_time_scale = 400.0;
    spec
}

/// A GRBL firmware on the GRBL emulator.
fn burn_on_grbl(fw: SimFirmware, corner: Corner) -> Vec<Burn> {
    burn_on_grbl_after(fw, corner, |_| {})
}

/// [`burn_on_grbl`], with `before` run on the connected machine before the job starts.
fn burn_on_grbl_after(fw: SimFirmware, corner: Corner, before: impl FnOnce(&mut DarkBurn)) -> Vec<Burn> {
    let what = format!("{fw:?} {corner:?}");
    let mut a = app_with_design(DeviceType::SimulatorGrbl, corner, Some(fw));
    let (w, h) = a.machine.active().bed();
    let (adapter, emu) = crate::connections::grbl_simulator(&spec_of(&a).config);
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGrbl);
    wait(&mut a, &format!("{what}: ready"), |a| a.machine.handle.snapshot().state == LaserState::Ready);
    before(&mut a);
    start_and_finish(&mut a, &what);
    emu.with(|e| {
        assert_eq!(e.stats().errors, 0, "{what}: the controller refused a line");
        assert!(!e.laser_on(), "{what}: the laser is off at the end");
        let s_max = e.setting(30).unwrap_or(1000.0);
        e.stats()
            .burns
            .iter()
            .map(|b| Burn {
                a: to_design(Point::new(b.from[0], b.from[1]), corner, w, h),
                b: to_design(Point::new(b.to[0], b.to[1]), corner, w, h),
                power: b.s / s_max * 100.0,
            })
            .collect()
    })
}

/// Smoothieware, Marlin, Snapmaker or a custom G-code machine on the line emulator.
fn burn_on_gcode(kind: SimGcode, corner: Corner) -> Vec<Burn> {
    let what = format!("{kind:?} {corner:?}");
    let ty = match kind {
        SimGcode::Smoothie => DeviceType::SimulatorSmoothie,
        SimGcode::Marlin => DeviceType::SimulatorMarlin,
        SimGcode::Snapmaker => DeviceType::SimulatorSnapmaker,
        _ => DeviceType::SimulatorCustom,
    };
    let mut a = app_with_design(ty, corner, None);
    let (w, h) = a.machine.active().bed();
    let (adapter, emu) = crate::gcode_machines::simulator_adapter(kind, &spec_of(&a).config);
    let emu = emu.expect("a line emulator");
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGcode(kind));
    start_and_finish(&mut a, &what);
    emu.with(|e| {
        assert!(!e.laser_on(), "{what}: the laser is off at the end");
        assert_eq!(e.stats().rapid_burn_mm, 0.0, "{what}: the laser was on during a rapid move");
        e.stats()
            .burns
            .iter()
            .map(|b| Burn {
                a: to_design(Point::new(b.from[0], b.from[1]), corner, w, h),
                b: to_design(Point::new(b.to[0], b.to[1]), corner, w, h),
                power: b.power * 100.0,
            })
            .collect()
    })
}

/// A Ruida controller on its emulator.
fn burn_on_ruida(corner: Corner) -> Vec<Burn> {
    let what = format!("Ruida {corner:?}");
    let mut a = app_with_design(DeviceType::SimulatorRuida, corner, None);
    let (w, h) = a.machine.active().bed();
    let spec = spec_of(&a);
    let cfg = ruida::EmulatorConfig { step_mode: true, bed_x_um: (w * 1000.0) as u32, bed_y_um: (h * 1000.0) as u32, ..Default::default() };
    let emu = Arc::new(Mutex::new(ruida::RuidaEmulator::new(cfg)));
    let transport: Box<dyn ruida::Transport> = Box::new(ruida::MemoryTransport::udp(emu.clone()));
    let adapter = RuidaAdapter::new(transport, &spec.config, true, Some((emu.clone(), spec.config.sim_time_scale)));
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorRuida);
    start_and_finish(&mut a, &what);
    let e = emu.lock().unwrap();
    let mm = |p: [i32; 4]| to_design(Point::new(f64::from(p[0]) / 1000.0, f64::from(p[1]) / 1000.0), corner, w, h);
    e.motion
        .iter()
        .filter(|m| m.kind == ruida::emulator::MotionKind::Cut && m.power.percent() > 0.0)
        .map(|m| Burn { a: mm(m.from), b: mm(m.to), power: m.power.percent() })
        .collect()
}

/// A galvo board on its emulator. The board's field is centred; the design's lower left corner
/// is the field's lower left corner.
fn burn_on_galvo() -> (Vec<Burn>, Vec<String>) {
    let mut a = app_with_design(DeviceType::SimulatorGalvo, Corner::BottomLeft, None);
    let dev = a.machine.active().clone();
    let settings = dev.settings.galvo.clone();
    let field = galvo::Field::new(settings.to_library().field);
    let (bed, _) = dev.bed();
    let (adapter, emu) = crate::galvo_machine::simulator_with_handle(Arc::new(settings));
    a.machine.handle.attach_simulator(Box::new(adapter), Link::SimulatorGalvo);
    wait(&mut a, "galvo ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let warnings = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap().warnings;
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    wait(&mut a, "armed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed));
    run(&mut a, "machine.start", Value::Null).unwrap();
    wait(&mut a, "galvo job finished", |a| {
        a.machine.last_events.iter().any(|e| matches!(e, MachineEvent::JobFinished(_)))
            || (emu.lock().unwrap().log().executed > 0 && a.machine.handle.snapshot().state == LaserState::Ready)
    });
    wait(&mut a, "galvo ready again", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let g = emu.lock().unwrap();
    assert!(!g.laser_enabled() && !g.laser_signal(), "galvo: the laser is off after the job");
    let mm = |p: (u16, u16)| {
        let q = field.galvo_to_mm(f64::from(p.0), f64::from(p.1));
        Point::new(q.x + bed / 2.0, q.y + bed / 2.0)
    };
    let burns =
        g.log().motions.iter().filter(|m| m.mark && m.emitting).map(|m| Burn { a: mm(m.from), b: mm(m.to), power: f64::from(m.current) }).collect();
    (burns, warnings)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn same_rect(a: Rect, b: Rect, tol: f64) -> bool {
    close(a.x0, b.x0, tol) && close(a.y0, b.y0, tol) && close(a.x1, b.x1, tol) && close(a.y1, b.y1, tol)
}

/// The three parts as they must be burned, whatever the machine.
fn check_against_the_design(what: &str, p: &[Part; 3]) {
    // The line: the rectangle grown by the kerf, two passes, each less its two 2 mm tabs. The
    // kerf rounds the corners (a quarter circle of 0.2 mm each).
    let once = 2.0 * (30.0 + 20.0) + std::f64::consts::TAU * 0.2 - 2.0 * 2.0;
    assert!(same_rect(p[0].bounds, Rect::new(19.8, 19.8, 50.2, 40.2), 0.02), "{what}: the line is burned at {:?}", p[0].bounds);
    assert!(close(p[0].length, 2.0 * once, 0.6), "{what}: the line burns {} mm, two passes of {once}", p[0].length);
    // The fill: ten lines of 20 mm, half a line in from the edges.
    assert!(same_rect(p[1].bounds, Rect::new(70.0, 20.5, 90.0, 29.5), 0.02), "{what}: the fill is burned at {:?}", p[1].bounds);
    assert!(close(p[1].length, 200.0, 0.5), "{what}: the fill burns {} mm", p[1].length);
    // The picture: sixteen lines, two black squares of 2 mm on each.
    assert!(same_rect(p[2].bounds, Rect::new(20.0, 60.25, 28.0, 67.75), 0.02), "{what}: the picture is burned at {:?}", p[2].bounds);
    assert!(close(p[2].length, 64.0, 0.5), "{what}: the picture burns {} mm", p[2].length);
}

fn check_power(what: &str, p: &[Part; 3]) {
    for (part, want) in p.iter().zip([60.0, 40.0, 50.0]) {
        assert!(close(part.power.0, want, 0.5) && close(part.power.1, want, 0.5), "{what}: burned at {:?} percent, set to {want}", part.power);
    }
}

#[test]
fn every_grbl_firmware_burns_the_same_job() {
    for fw in [SimFirmware::Grbl11h, SimFirmware::Grbl11e, SimFirmware::Grbl09, SimFirmware::GrblLpc, SimFirmware::GrblHal, SimFirmware::FluidNc] {
        let p = parts(&burn_on_grbl(fw, Corner::BottomLeft));
        check_against_the_design(&format!("{fw:?}"), &p);
        check_power(&format!("{fw:?}"), &p);
    }
}

/// A GRBL 1.1 controller whose laser mode is switched off (`$32=0`) keeps its output on at the
/// last S through rapid moves, and reads M4 as "spindle counter-clockwise".
#[test]
fn a_grbl_controller_with_laser_mode_off_burns_the_same_job() {
    let burns = burn_on_grbl_after(SimFirmware::Grbl11h, Corner::BottomLeft, |a| {
        a.machine.handle.write_setting(32, "0").unwrap();
        a.machine.handle.read_settings().unwrap();
        wait(a, "laser mode off", |a| a.machine.handle.snapshot().grbl_settings.get(&32).is_some_and(|v| v.trim() == "0"));
    });
    let p = parts(&burns);
    check_against_the_design("GRBL 1.1h with $32=0", &p);
    check_power("GRBL 1.1h with $32=0", &p);
}

#[test]
fn the_job_lands_in_the_same_place_for_every_origin_corner() {
    for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomRight] {
        let p = parts(&burn_on_grbl(SimFirmware::Grbl11h, corner));
        check_against_the_design(&format!("GRBL {corner:?}"), &p);
        let p = parts(&burn_on_ruida(corner));
        check_against_the_design(&format!("Ruida {corner:?}"), &p);
        let p = parts(&burn_on_gcode(SimGcode::Marlin, corner));
        check_against_the_design(&format!("Marlin {corner:?}"), &p);
    }
}

#[test]
fn the_g_code_machines_burn_the_same_job() {
    for kind in [SimGcode::Smoothie, SimGcode::Marlin, SimGcode::Snapmaker, SimGcode::Custom] {
        let p = parts(&burn_on_gcode(kind, Corner::BottomLeft));
        check_against_the_design(&format!("{kind:?}"), &p);
        // The plain emulator behind the custom device counts S against 255 whatever the S max
        // of the device (1000 by default), so its power cannot be compared.
        if kind != SimGcode::Custom {
            check_power(&format!("{kind:?}"), &p);
        }
    }
}

#[test]
fn a_ruida_controller_burns_the_same_job() {
    let p = parts(&burn_on_ruida(Corner::BottomLeft));
    check_against_the_design("Ruida", &p);
    check_power("Ruida", &p);
}

#[test]
fn a_galvo_marks_the_line_and_the_fill_of_the_same_job() {
    let (burns, warnings) = burn_on_galvo();
    let p = parts(&burns);
    // A galvo field is 65536 steps across: 110 mm / 65536 is under two thousandths.
    let once = 2.0 * (30.0 + 20.0) + std::f64::consts::TAU * 0.2 - 2.0 * 2.0;
    assert!(same_rect(p[0].bounds, Rect::new(19.8, 19.8, 50.2, 40.2), 0.02), "galvo: the line is marked at {:?}", p[0].bounds);
    assert!(close(p[0].length, 2.0 * once, 0.6), "galvo: the line marks {} mm, two passes of {once}", p[0].length);
    assert!(same_rect(p[1].bounds, Rect::new(70.0, 20.5, 90.0, 29.5), 0.02), "galvo: the fill is marked at {:?}", p[1].bounds);
    assert!(close(p[1].length, 200.0, 0.5), "galvo: the fill marks {} mm", p[1].length);
    // Power is a current word on the board: the line at 60 percent, the fill at 40.
    assert!(close(p[0].power.0, p[0].power.1, 0.5) && close(p[1].power.0, p[1].power.1, 0.5), "{:?} {:?}", p[0].power, p[1].power);
    assert!(close(p[0].power.1 / p[1].power.1, 1.5, 0.01), "line {:?} against fill {:?}", p[0].power, p[1].power);
    // A picture on a galvo is engraved in depth slices, another thing than the dots of the
    // other machines; it stays inside the picture.
    if p[2].length > 0.0 {
        let b = p[2].bounds;
        assert!(b.x0 > 19.9 && b.y0 > 59.9 && b.x1 < 28.1 && b.y1 < 68.1, "galvo: the picture is marked at {b:?}; {warnings:?}");
    }
}
