//! Arrange and transform commands on machines with their zero in each of the four corners, on a
//! text, a sheared shape and a picture: after every command, what the job sends (machine
//! coordinates, brought back to the design by hand) must be what the canvas shows (the shapes'
//! own outlines and the picture's pixels through its transform), and the canvas must not depend
//! on the corner at all. The simulator's head is sent to the selection as well.
//!
//! Hardware safety: jobs are only built here; the one connection is to the in-memory GRBL
//! simulator.

use std::sync::Arc;
use std::time::{Duration, Instant};

use darkburn_core::arrange::{self, Anchor};
use darkburn_core::kurbo::{Affine, Point, Rect};
use darkburn_core::layer::{ImageMode, Mode};
use darkburn_core::{ImageShape, Shape, ShapeId, ShapeKind};
use darkburn_job::{Corner, Op};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const CORNERS: [Corner; 4] = [Corner::BottomLeft, Corner::TopLeft, Corner::TopRight, Corner::BottomRight];

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

struct Design {
    text: ShapeId,
    sheared: ShapeId,
    picture: ShapeId,
}

impl Design {
    fn all(&self) -> Vec<ShapeId> {
        vec![self.text, self.sheared, self.picture]
    }
}

/// The black block of the picture in the picture's own millimetres: pixels 0..4 across and down
/// of 16 x 8, half a millimetre each, the picture centred on its own zero with y up.
const BLOCK: [(f64, f64); 4] = [(-4.0, 2.0), (-2.0, 2.0), (-2.0, 0.0), (-4.0, 0.0)];

fn app_at(corner: Corner) -> (DarkBurn, Design) {
    let mut a = DarkBurn::new(Prefs::default());
    let id = run(&mut a, "device.add", json!({ "type": "SimulatorGrbl" })).unwrap()["id"].as_u64().unwrap();
    a.machine.devices.get_mut(id).unwrap().origin = corner;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    crate::machine_ui::adopt_device_state(&mut a);
    crate::machine_ui::sync_job_state(&mut a);
    {
        let s = a.editor.doc.layer_mut(0).main_mut();
        s.mode = Mode::Line;
        s.speed = 100.0;
        s.max_power = 50.0;
        s.min_power = 50.0;
    }
    {
        let s = a.editor.doc.layer_mut(2).main_mut();
        s.mode = Mode::Image;
        s.image_mode = ImageMode::Threshold;
        s.speed = 100.0;
        s.max_power = 50.0;
        s.min_power = 50.0;
        s.interval = 0.1;
        s.overscan = 0.0;
    }
    let text = run(&mut a, "text.add", json!({ "text": "F", "x": 40, "y": 40, "height": 10, "font": "IBM Plex Sans", "layer": 0 })).unwrap();
    let text = ShapeId(text["id"].as_u64().unwrap());
    let sheared = run(&mut a, "shape.add_rect", json!({ "x": 60, "y": 40, "width": 20, "height": 10, "layer": 0 })).unwrap();
    let sheared = ShapeId(sheared["id"].as_u64().unwrap());
    a.editor.edit("Shear", |d, _| arrange::transform(d, &[sheared], Affine::new([1.0, 0.0, 0.4, 1.0, 0.0, 0.0])));
    let px: Vec<u8> = (0..16 * 8).map(|i| if i % 16 < 4 && i / 16 < 4 { 0 } else { 255 }).collect();
    let img = image::GrayImage::from_raw(16, 8, px).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let key = a.editor.doc.add_image(darkburn_core::ImageData { format: "png".into(), bytes: Arc::from(png), px_width: 16, px_height: 8 });
    let picture = a.editor.doc.add(Shape::new(
        ShapeKind::Image(ImageShape { image: key, width: 8.0, height: 4.0, ..Default::default() }),
        Affine::translate((100.0, 60.0)),
        2,
    ));
    (a, Design { text, sheared, picture })
}

/// Machine coordinates back to the design for a bed of `w` x `h` with its zero at `corner`.
fn to_design(p: Point, corner: Corner, w: f64, h: f64) -> Point {
    match corner {
        Corner::BottomLeft => p,
        Corner::TopLeft => Point::new(p.x, h - p.y),
        Corner::TopRight => Point::new(w - p.x, h - p.y),
        Corner::BottomRight => Point::new(w - p.x, p.y),
    }
}

fn to_machine(p: Point, corner: Corner, w: f64, h: f64) -> Point {
    // The mapping is its own inverse.
    to_design(p, corner, w, h)
}

fn grow(r: Option<Rect>, p: Point) -> Option<Rect> {
    Some(r.map_or(Rect::from_points(p, p), |r| r.union_pt(p)))
}

/// Where the job burns, in design coordinates: (the line layer, the picture layer).
fn burned(a: &DarkBurn, corner: Corner) -> (Rect, Rect) {
    let (w, h) = a.machine.active().bed();
    let job = crate::job_cmds::build_now(a).unwrap_or_else(|e| panic!("{corner:?}: {e}"));
    let tp = &job.toolpath;
    let (mut lines, mut picture) = (None, None);
    let mut pos = tp.start;
    let mut layer = 0;
    for op in &tp.ops {
        match op {
            Op::LayerStart { layer: l } => layer = tp.layers[*l].layer_index,
            Op::Rapid { to } | Op::Overscan { to, .. } => pos = *to,
            Op::Cut { to, power, .. } | Op::Arc { to, power, .. } => {
                if *power > 0.0 {
                    let into = if layer == 2 { &mut picture } else { &mut lines };
                    for p in [pos, *to] {
                        *into = grow(*into, to_design(p, corner, w, h));
                    }
                }
                pos = *to;
            }
            _ => {}
        }
    }
    (lines.expect("the line layer burns"), picture.expect("the picture burns"))
}

/// What the canvas shows: (the outlines of the text and the sheared shape, the black block).
fn shown(a: &DarkBurn, d: &Design) -> (Rect, Rect) {
    let doc = &a.editor.doc;
    let lines = doc.get(d.text).unwrap().bbox().union(doc.get(d.sheared).unwrap().bbox());
    let t = doc.get(d.picture).unwrap().transform;
    let block = BLOCK.iter().fold(None, |r, (x, y)| grow(r, t * Point::new(*x, *y))).unwrap();
    (lines, block)
}

fn near(a: Rect, b: Rect, tol: f64) -> bool {
    (a.x0 - b.x0).abs() <= tol && (a.y0 - b.y0).abs() <= tol && (a.x1 - b.x1).abs() <= tol && (a.y1 - b.y1).abs() <= tol
}

/// The job against the canvas. Lines are exact. A picture is burned in scan lines a tenth of a
/// millimetre apart from pixels of half a millimetre, and the edge of a black area lies up to a
/// fifth of a millimetre inside where it is drawn when the pixels do not sit on the scan lines
/// (the tones between two pixels are blended, and the lighter one wins).
fn check(a: &DarkBurn, d: &Design, corner: Corner, what: &str, picture_tol: f64) -> (Rect, Rect) {
    let (lines, block) = shown(a, d);
    let (job_lines, job_block) = burned(a, corner);
    assert!(near(job_lines, lines, 0.02), "{corner:?} after {what}: the lines are shown at {lines:?} and burned at {job_lines:?}");
    assert!(near(job_block, block, picture_tol), "{corner:?} after {what}: the black block is shown at {block:?} and burned at {job_block:?}");
    (lines, block)
}

#[test]
fn arrange_commands_burn_what_the_canvas_shows_for_every_origin_corner() {
    let steps: Vec<(&str, Value)> = vec![
        ("arrange.flip_h", json!({})),
        ("arrange.flip_v", json!({})),
        ("arrange.rotate_cw", json!({})),
        ("arrange.page_upper_left", json!({})),
        ("arrange.rotate_ccw", json!({})),
        ("arrange.page_lower_right", json!({})),
        ("arrange.page_upper_right", json!({})),
        ("arrange.page_lower_left", json!({})),
        ("arrange.page_centre", json!({})),
        ("arrange.distribute_h", json!({})),
        ("arrange.align_left", json!({})),
        ("arrange.align_top", json!({})),
        ("arrange.align_centres", json!({})),
        ("shape.translate", json!({ "dx": 12.5, "dy": -7.25 })),
        ("arrange.rotate_by", json!({ "degrees": 30.0 })),
    ];
    let mut seen: Vec<Vec<(Rect, Rect)>> = Vec::new();
    for corner in CORNERS {
        let (mut a, d) = app_at(corner);
        let (w, h) = a.machine.active().bed();
        assert_eq!((f64::from(a.prefs.page_width), f64::from(a.prefs.page_height)), (w, h), "the canvas shows the device's bed");
        run(&mut a, "selection.set", json!({ "ids": d.all().iter().map(|i| i.0).collect::<Vec<_>>() })).unwrap();
        let mut results = vec![check(&a, &d, corner, "nothing", 0.3)];
        for (id, args) in &steps {
            let before = a.editor.doc.bbox_of(&d.all()).unwrap();
            run(&mut a, id, args.clone()).unwrap_or_else(|e| panic!("{corner:?}: {id}: {e}"));
            let bb = a.editor.doc.bbox_of(&d.all()).unwrap();
            // What the command means on the page, whichever corner the machine counts from.
            let ok = match *id {
                "arrange.flip_h" | "arrange.flip_v" => near(bb, before, 1e-6),
                "arrange.page_upper_left" => (bb.x0, bb.y1) == (0.0, h) || near(bb, Rect::new(0.0, h - bb.height(), bb.width(), h), 1e-6),
                "arrange.page_lower_right" => near(bb, Rect::new(w - bb.width(), 0.0, w, bb.height()), 1e-6),
                "arrange.page_upper_right" => near(bb, Rect::new(w - bb.width(), h - bb.height(), w, h), 1e-6),
                "arrange.page_lower_left" => near(bb, Rect::new(0.0, 0.0, bb.width(), bb.height()), 1e-6),
                "arrange.page_centre" => (bb.center() - Point::new(w / 2.0, h / 2.0)).hypot() < 1e-6,
                "arrange.align_left" => d.all().iter().all(|i| (a.editor.doc.get(*i).unwrap().bbox().x0 - bb.x0).abs() < 1e-6),
                "arrange.align_top" => d.all().iter().all(|i| (a.editor.doc.get(*i).unwrap().bbox().y1 - bb.y1).abs() < 1e-6),
                "shape.translate" => near(bb, before + darkburn_core::kurbo::Vec2::new(12.5, -7.25), 1e-6),
                _ => true,
            };
            assert!(ok, "{corner:?}: {id} left the selection at {bb:?} (it was at {before:?}) on a {w} x {h} page");
            // The corners of a block turned by 30 degrees are points too fine to burn.
            let tol = if *id == "arrange.rotate_by" { 0.5 } else { 0.3 };
            results.push(check(&a, &d, corner, id, tol));
        }
        seen.push(results);
    }
    for (corner, results) in CORNERS.iter().zip(&seen).skip(1) {
        for (i, (got, first)) in results.iter().zip(&seen[0]).enumerate() {
            let what = if i == 0 { "nothing" } else { steps[i - 1].0 };
            assert!(
                near(got.0, first.0, 1e-6) && near(got.1, first.1, 1e-6),
                "{corner:?} after {what}: the canvas differs from the bottom-left machine's"
            );
        }
    }
}

/// Flip puts the picture's black block on the other side, in the job as on the canvas.
#[test]
fn a_flipped_picture_is_burned_flipped() {
    for corner in CORNERS {
        let (mut a, d) = app_at(corner);
        run(&mut a, "selection.set", json!({ "ids": [d.picture.0] })).unwrap();
        let (_, block) = burned(&a, corner);
        assert!(near(block, Rect::new(96.0, 60.0, 98.0, 62.0), 0.3), "{corner:?}: {block:?}");
        run(&mut a, "arrange.flip_h", json!({})).unwrap();
        let (_, block) = burned(&a, corner);
        assert!(near(block, Rect::new(102.0, 60.0, 104.0, 62.0), 0.3), "{corner:?} flipped across: {block:?}");
        run(&mut a, "arrange.flip_v", json!({})).unwrap();
        let (_, block) = burned(&a, corner);
        assert!(near(block, Rect::new(102.0, 58.0, 104.0, 60.0), 0.3), "{corner:?} flipped both ways: {block:?}");
        check(&a, &d, corner, "both flips", 0.3);
    }
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// "Move laser to selection": the head goes where the job burns that point of the selection, and
/// "move selection to laser" brings the selection back under it, for every corner.
#[test]
fn the_head_goes_to_the_selection_for_every_origin_corner() {
    for corner in CORNERS {
        let (mut a, d) = app_at(corner);
        let (w, h) = a.machine.active().bed();
        assert_eq!(a.machine.active().kind, DeviceType::SimulatorGrbl);
        run(&mut a, "machine.connect", json!({})).unwrap();
        wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        run(&mut a, "selection.set", json!({ "ids": [d.sheared.0] })).unwrap();
        let bb = a.editor.doc.get(d.sheared).unwrap().bbox();
        for (id, anchor) in [
            ("arrange.laser_upper_left", Anchor::TopLeft),
            ("arrange.laser_lower_right", Anchor::BottomRight),
            ("arrange.laser_top", Anchor::Top),
            ("arrange.laser_centre", Anchor::Centre),
        ] {
            let want = to_machine(anchor.of(bb), corner, w, h);
            run(&mut a, id, json!({})).unwrap_or_else(|e| panic!("{corner:?}: {id}: {e}"));
            wait(&mut a, id, |a| {
                let p = a.machine.handle.snapshot().machine_pos;
                (p[0] - want.x).abs() < 0.05 && (p[1] - want.y).abs() < 0.05
            });
            wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        }
        // The head is at the centre of the sheared shape. Bring the text's lower left corner there.
        run(&mut a, "machine.get_position", json!({})).unwrap();
        wait(&mut a, "position known", |a| a.machine.handle.snapshot().has_position);
        run(&mut a, "selection.set", json!({ "ids": [d.text.0] })).unwrap();
        run(&mut a, "arrange.to_laser", json!({ "anchor": "bottom_left" })).unwrap();
        let t = a.editor.doc.get(d.text).unwrap().bbox();
        assert!((Point::new(t.x0, t.y0) - bb.center()).hypot() < 0.05, "{corner:?}: the text went to {t:?}, the head is over {:?}", bb.center());
        check(&a, &d, corner, "move to laser", 0.3);
    }
}
