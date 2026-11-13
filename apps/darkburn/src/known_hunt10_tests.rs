//! Bug hunt 10, four known bugs in the app: a device's S max that is not a number, Save as with
//! no file name, the node tool's join on release, and Ctrl+Z in the middle of a canvas drag.
//! Every test runs without a window and opens no port, device or network connection.

use darkburn_core::kurbo::{Point, Rect as KRect};
use darkburn_core::{Shape, ShapeId};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::{ConnectionKind, Device, DeviceType};
use crate::prefs::Prefs;
use crate::render::Xf;
use crate::shell::DarkBurn;

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

// 1. S max -----------------------------------------------------------------------------------

#[test]
fn a_grbl_device_refuses_an_s_max_that_is_not_a_finite_number_above_zero() {
    for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 0.0, -5.0] {
        let mut d = Device::new(DeviceType::Grbl);
        // A link that is only described, never opened: connect_spec builds the plan, no more.
        d.connection = ConnectionKind::Tcp;
        d.host = "127.0.0.1".into();
        d.settings.gcode.s_max = bad;
        assert!(d.validate().iter().any(|p| p.contains("S max")), "validate lets S max {bad} through: {:?}", d.validate());
        assert!(d.connect_spec(&[]).is_err(), "connect_spec passes S max {bad} on to the driver");
    }
    let mut d = Device::new(DeviceType::Grbl);
    d.connection = ConnectionKind::Tcp;
    d.host = "127.0.0.1".into();
    d.settings.gcode.s_max = 1000.0;
    assert!(!d.validate().iter().any(|p| p.contains("S max")));
    assert_eq!(d.connect_spec(&[]).unwrap().config.s_max, Some(1000.0));
}

// 2. Save as with no name ----------------------------------------------------------------------

#[test]
fn save_as_with_an_empty_or_blank_path_is_refused_and_writes_nothing() {
    let mut a = app();
    for path in ["", "   "] {
        assert!(run(&mut a, "library.save_as", json!({ "path": path })).is_err(), "library.save_as {path:?} answered ok");
        let lib = run(&mut a, "art.new", json!({})).unwrap()["library"].clone();
        assert!(run(&mut a, "art.save_as", json!({ "library": lib, "path": path })).is_err(), "art.save_as {path:?} answered ok");
    }
    // Where an empty name would have written: the current folder.
    assert!(!std::path::Path::new(".dbclb").exists(), "a file named .dbclb was written");
    assert!(!std::path::Path::new(".dbart").exists(), "a file named .dbart was written");
    assert!(!std::path::Path::new("   .dbclb").exists());
    assert!(!std::path::Path::new("   .dbart").exists());
    assert!(a.extras.library.path.is_none(), "the library took no file");
}

// Canvas rig: real egui frames through the same code as the window ---------------------------

const SCREEN: (f32, f32) = (1200.0, 900.0);

struct Rig {
    ctx: egui::Context,
    app: DarkBurn,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut rig = Rig { ctx: egui::Context::default(), app: app(), time: 0.0 };
        rig.frame(vec![], Modifiers::NONE);
        rig
    }

    fn frame(&mut self, mut events: Vec<Event>, modifiers: Modifiers) {
        self.time += 0.016;
        events.insert(0, Event::ModifiersChanged(modifiers));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SCREEN.0, SCREEN.1))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| {
            app.shortcuts(ui.ctx());
            let cmd = app.view_cmd.take();
            crate::canvas::show(ui, app, cmd);
        });
        out.textures_delta.clear();
    }

    fn screen(&self, p: Point) -> Pos2 {
        let canvas = Rect::from_min_max(Pos2::new(crate::canvas::RULER, crate::canvas::RULER), Pos2::new(SCREEN.0, SCREEN.1));
        Xf::new(canvas, &self.app.prefs.view, self.app.prefs.page().y).to_screen(p)
    }

    fn mv(&mut self, to: Pos2) {
        self.frame(vec![Event::PointerMoved(to)], Modifiers::NONE);
    }

    fn button(&mut self, at: Pos2, pressed: bool) {
        self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }], Modifiers::NONE);
    }

    fn ctrl_z(&mut self) {
        let m = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        self.frame(vec![Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: m }], m);
        self.frame(vec![Event::Key { key: Key::Z, physical_key: None, pressed: false, repeat: false, modifiers: m }], Modifiers::NONE);
    }
}

// 3. Node tool join ------------------------------------------------------------------------------

// 4. Ctrl+Z during a drag ------------------------------------------------------------------------

fn rect_rig() -> (Rig, ShapeId) {
    let mut rig = Rig::new();
    let mut id = ShapeId(0);
    rig.app.editor.edit("Add", |d, sel| {
        id = d.add(Shape::rect(KRect::new(100.0, 100.0, 140.0, 130.0), 0.0, 0));
        sel.set([id]);
        true
    });
    rig.frame(vec![], Modifiers::NONE);
    (rig, id)
}

#[test]
fn ctrl_z_during_a_drag_that_moves_nothing_keeps_the_step_before_it() {
    let (mut rig, id) = rect_rig();
    let a = rig.screen(Point::new(120.0, 115.0));
    rig.mv(a);
    rig.button(a, true);
    rig.mv(a);
    rig.ctrl_z();
    rig.button(a, false);
    rig.frame(vec![], Modifiers::NONE);
    assert!(rig.app.editor.doc.get(id).is_some(), "the shape is still there");
    assert_eq!(rig.app.editor.history.undo_label(), Some("Add"), "the step before the drag is still in the history");
}
