//! Canvas behaviour driven through real egui frames: pointer and key events go through the same
//! code as in the window (`DarkBurn::shortcuts`, then `canvas::show`).

use darkburn_core::kurbo::{Point, Rect as KRect};
use darkburn_core::{Shape, ShapeId};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};

use crate::prefs::Prefs;
use crate::render::Xf;
use crate::shell::DarkBurn;

const SCREEN: (f32, f32) = (1200.0, 900.0);

#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

struct Rig {
    ctx: egui::Context,
    app: DarkBurn,
    time: f64,
}

impl Rig {
    fn new() -> Rig {
        let mut rig = Rig { ctx: egui::Context::default(), app: DarkBurn::new(Prefs::default()), time: 0.0 };
        rig.frame(vec![], Modifiers::NONE);
        rig
    }

    fn frame(&mut self, events: Vec<Event>, _modifiers: Modifiers) {
        self.time += 0.016;
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

    fn canvas(&self) -> Rect {
        Rect::from_min_max(Pos2::new(crate::canvas::RULER, crate::canvas::RULER), Pos2::new(SCREEN.0, SCREEN.1))
    }

    fn xf(&self) -> Xf {
        Xf::new(self.canvas(), &self.app.prefs.view, self.app.prefs.page().y)
    }

    fn screen(&self, p: Point) -> Pos2 {
        self.xf().to_screen(p)
    }

    fn mv(&mut self, to: Pos2) {
        self.frame(vec![Event::PointerMoved(to)], Modifiers::NONE);
    }

    fn button(&mut self, at: Pos2, b: PointerButton, pressed: bool) {
        self.frame(vec![Event::PointerButton { pos: at, button: b, pressed, modifiers: Modifiers::NONE }], Modifiers::NONE);
    }

    fn key(&mut self, key: Key, m: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: m }], m);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: m }], m);
    }

    fn add_rect(&mut self, r: KRect, layer: usize) -> ShapeId {
        let mut id = ShapeId(0);
        self.app.editor.edit("Add", |d, sel| {
            id = d.add(Shape::rect(r, 0.0, layer));
            sel.set([id]);
            true
        });
        id
    }

    fn rotation_deg(&self, id: ShapeId) -> f64 {
        let c = self.app.editor.doc.get(id).unwrap().transform.as_coeffs();
        c[1].atan2(c[0]).to_degrees()
    }
}

#[test]
fn comma_and_period_rotate_by_one_five_and_ten_degrees_and_shifted_by_ninety() {
    let mut rig = Rig::new();
    let id = rig.add_rect(KRect::new(100.0, 100.0, 140.0, 120.0), 0);
    let at = rig.screen(Point::new(10.0, 10.0));
    rig.mv(at);
    rig.key(Key::Comma, Modifiers::NONE);
    assert!((rig.rotation_deg(id) - 1.0).abs() < 1e-6, "{}", rig.rotation_deg(id));
    rig.key(Key::Period, Modifiers::COMMAND);
    assert!((rig.rotation_deg(id) - -4.0).abs() < 1e-6, "{}", rig.rotation_deg(id));
    rig.key(Key::Comma, Modifiers::ALT);
    assert!((rig.rotation_deg(id) - 6.0).abs() < 1e-6, "{}", rig.rotation_deg(id));
    rig.key(Key::Period, Modifiers::SHIFT);
    assert!((rig.rotation_deg(id) - -84.0).abs() < 1e-6, "{}", rig.rotation_deg(id));
}

#[test]
fn dragging_a_shape_to_the_edge_pans_the_view() {
    let mut rig = Rig::new();
    rig.add_rect(KRect::new(100.0, 100.0, 140.0, 120.0), 0);
    let start = rig.screen(Point::new(120.0, 110.0));
    rig.mv(start);
    rig.button(start, PointerButton::Primary, true);
    let before = rig.app.prefs.view;
    let edge = Pos2::new(rig.canvas().right() - 4.0, start.y);
    let mut p = start;
    while p.x < edge.x {
        p.x = (p.x + 40.0).min(edge.x);
        rig.mv(p);
    }
    for _ in 0..10 {
        rig.mv(edge);
    }
    assert!(rig.app.prefs.view != before, "the view moved while holding the shape at the edge");
    rig.button(edge, PointerButton::Primary, false);
}

#[test]
fn double_middle_click_frames_the_design() {
    let mut rig = Rig::new();
    rig.add_rect(KRect::new(10.0, 10.0, 40.0, 30.0), 0);
    let before = rig.app.prefs.view;
    let at = Pos2::new(600.0, 400.0);
    rig.mv(at);
    for _ in 0..2 {
        rig.button(at, PointerButton::Middle, true);
        rig.button(at, PointerButton::Middle, false);
    }
    rig.frame(vec![], Modifiers::NONE);
    rig.frame(vec![], Modifiers::NONE);
    assert!(rig.app.prefs.view.zoom > before.zoom * 2.0, "zoomed in on the small design: {} -> {}", before.zoom, rig.app.prefs.view.zoom);
}

#[test]
fn the_full_screen_cursor_option_draws_lines_across_the_canvas() {
    let mut rig = Rig::new();
    let at = Pos2::new(600.0, 400.0);
    rig.mv(at);
    let count = |rig: &mut Rig| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SCREEN.0, SCREEN.1))),
            events: vec![Event::PointerMoved(at)],
            ..Default::default()
        };
        let app = &mut rig.app;
        let mut out = rig.ctx.run_ui(input, |ui| {
            crate::canvas::show(ui, app, None);
        });
        out.textures_delta.clear();
        out.shapes.len()
    };
    rig.app.prefs.opts.fullscreen_cursor = false;
    let _ = count(&mut rig);
    let off = count(&mut rig);
    rig.app.prefs.opts.fullscreen_cursor = true;
    let on = count(&mut rig);
    assert!(on > off, "the crosshair adds shapes: {off} -> {on}");
}

fn click(rig: &mut Rig, at: Pos2) {
    rig.mv(at);
    rig.button(at, PointerButton::Primary, true);
    rig.button(at, PointerButton::Primary, false);
    rig.frame(vec![], Modifiers::NONE);
}

#[test]
fn locked_and_hidden_shapes_cannot_be_picked_by_clicking() {
    let mut rig = Rig::new();
    let id = rig.add_rect(KRect::new(100.0, 100.0, 140.0, 120.0), 3);
    rig.app.editor.selection.clear();
    let at = rig.screen(Point::new(100.0, 110.0));
    click(&mut rig, at);
    assert_eq!(rig.app.editor.selected(), vec![id], "an ordinary click selects");

    rig.app.editor.selection.clear();
    rig.app.editor.edit("Lock", |d, _| {
        d.get_mut(id).unwrap().locked = true;
        true
    });
    click(&mut rig, at);
    assert!(rig.app.editor.selected().is_empty(), "locked shapes are not selectable");

    rig.app.editor.edit("Unlock", |d, _| {
        d.get_mut(id).unwrap().locked = false;
        true
    });
    rig.app.editor.doc.layer_mut(3).show = false;
    click(&mut rig, at);
    assert!(rig.app.editor.selected().is_empty(), "shapes on hidden layers are not selectable");
}

#[test]
fn dropping_a_device_profile_and_an_image_on_the_window_adds_them() {
    let dir = std::env::temp_dir().join(format!("darkburn-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut rig = Rig::new();
    // A real profile exported from the active device.
    let id = rig.app.machine.devices.active().unwrap().id;
    let text = rig.app.machine.devices.export_profile(id).unwrap();
    let profile = dir.join("other.dbdev");
    std::fs::write(&profile, text).unwrap();
    let svg = dir.join("box.svg");
    std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm"><rect width="20" height="10"/></svg>"#).unwrap();
    let devices_before = rig.app.machine.devices.devices.len();
    let shapes_before = rig.app.editor.doc.len();
    let files = [profile, svg]
        .into_iter()
        .map(|p| std::sync::Arc::new(Dropped(p)) as egui::DroppedFileHandle)
        .collect::<Vec<_>>();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SCREEN.0, SCREEN.1))),
        dropped_files: files,
        ..Default::default()
    };
    let app = &mut rig.app;
    let mut out = rig.ctx.run_ui(input, |ui| crate::files::frame(ui.ctx(), app));
    out.textures_delta.clear();
    // The import runs on a worker thread.
    for _ in 0..200 {
        crate::file_io::frame(&rig.ctx, &mut rig.app);
        if rig.app.editor.doc.len() > shapes_before {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(rig.app.machine.devices.devices.len(), devices_before + 1, "the profile was added");
    assert!(rig.app.editor.doc.len() > shapes_before, "the artwork was imported");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn click_tolerance_decides_how_close_a_line_must_be() {
    let mut doc = darkburn_core::Document::new();
    let mut path = darkburn_core::kurbo::BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((50.0, 0.0));
    let id = doc.add(Shape::path(path, 0));
    assert_eq!(darkburn_core::select::hit(&doc, Point::new(25.0, 0.4), 0.5), Some(id));
    assert_eq!(darkburn_core::select::hit(&doc, Point::new(25.0, 0.4), 0.2), None);
}
