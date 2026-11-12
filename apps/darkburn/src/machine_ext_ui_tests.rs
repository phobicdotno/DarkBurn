//! The window side of the G-code, galvo and Gweike pieces: every new panel and dialog draws, and
//! the things that must be plain on screen (the arm state, the Gweike capture note) are there.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn walk(s: &egui::epaint::Shape, found: &mut Vec<String>) {
    match s {
        egui::epaint::Shape::Text(t) => found.push(t.galley.text().to_string()),
        egui::epaint::Shape::Vec(v) => v.iter().for_each(|x| walk(x, found)),
        _ => {}
    }
}

/// Draw one frame of the machine panels and dialogs; the text on screen.
fn draw(ctx: &egui::Context, app: &mut DarkBurn) -> Vec<String> {
    draw_on(ctx, app, egui::vec2(1400.0, 1000.0))
}

/// The texts drawn on a screen of `size`.
fn draw_on(ctx: &egui::Context, app: &mut DarkBurn, size: egui::Vec2) -> Vec<String> {
    let mut found = Vec::new();
    // Windows size themselves on their first frame, so draw a few.
    for _ in 0..4 {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            crate::machine_ui::frame(ui.ctx(), app);
            for (i, id) in ["laser", "move", "console", "file_list"].into_iter().enumerate() {
                ui.push_id(i, |ui| {
                    egui::ScrollArea::vertical().id_salt(id).show(ui, |ui| {
                        crate::panels::contents(ui, id, app);
                    });
                });
            }
            crate::devices_ui::dialogs(ui.ctx(), app);
        });
        out.textures_delta.clear();
        found.clear();
        for s in &out.shapes {
            walk(&s.shape, &mut found);
        }
    }
    assert!(!found.is_empty());
    found
}

fn add(app: &mut DarkBurn, ty: DeviceType) -> u64 {
    let id = run(app, "device.add", json!({ "type": format!("{ty:?}") })).unwrap()["id"].as_u64().unwrap();
    run(app, "device.select", json!({ "id": id })).unwrap();
    id
}

#[test]
fn the_wizard_lists_every_type_and_greys_out_the_ones_that_need_a_capture() {
    let ctx = egui::Context::default();
    let mut a = DarkBurn::new(Prefs::default());
    a.machine.dlg.wizard = Some(crate::devices::Wizard::new());
    let t = draw(&ctx, &mut a);
    // The list scrolls; the first entries are on screen.
    for want in ["GRBL", "Smoothieware", "Marlin", "Snapmaker (USB serial)"] {
        assert!(t.iter().any(|x| x == want), "{want} in the wizard: {t:?}");
    }
}

#[test]
fn gcode_pages_and_machine_settings_draw() {
    let ctx = egui::Context::default();
    let mut a = DarkBurn::new(Prefs::default());
    for ty in [DeviceType::SimulatorSmoothie, DeviceType::CustomGcode, DeviceType::LinuxCnc, DeviceType::Snapmaker, DeviceType::SimulatorMarlin] {
        add(&mut a, ty);
        a.machine.dlg.settings = Some((a.machine.active().clone(), 1));
        draw(&ctx, &mut a);
        a.machine.dlg.settings = None;
    }
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorSmoothie).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "machine.read_settings", Value::Null).unwrap();
    wait(&mut a, "settings", |a| !a.machine.handle.snapshot().gcode_settings.is_empty());
    a.machine.dlg.machine_settings = true;
    let t = draw(&ctx, &mut a);
    assert!(t.iter().any(|x| x == "Store in the controller"), "{t:?}");
}

#[test]
fn galvo_panel_shows_the_arm_state_and_every_galvo_dialog_draws() {
    let ctx = egui::Context::default();
    let mut a = DarkBurn::new(Prefs::default());
    add(&mut a, DeviceType::SimulatorGalvo);
    draw(&ctx, &mut a);
    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "galvo ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let t = draw(&ctx, &mut a);
    assert!(t.iter().any(|x| x == "Arm the laser"), "the arm button is on the Laser panel: {t:?}");
    assert!(t.iter().any(|x| x.starts_with("Not armed")), "{t:?}");
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    wait(&mut a, "armed", |a| a.machine.handle.snapshot().galvo.as_ref().is_some_and(|g| g.armed));
    let t = draw(&ctx, &mut a);
    assert!(t.iter().any(|x| x.starts_with("ARMED")), "armed is shown plainly: {t:?}");
    a.machine.dlg.settings = Some((a.machine.active().clone(), 4));
    a.machine.ext.galvo_frame_open = true;
    a.machine.ext.galvo_lens_open = true;
    a.machine.ext.galvo_repeat_open = true;
    let t = draw(&ctx, &mut a);
    assert!(t.iter().any(|x| x == "Red-light frame") && t.iter().any(|x| x == "Repeat marking"), "{t:?}");
    // The menu entries open the dialogs.
    a.machine.ext.galvo_frame_open = false;
    a.machine.ext.galvo_lens_open = false;
    a.machine.ext.galvo_repeat_open = false;
    run(&mut a, "laser.lens_test", Value::Null).unwrap();
    run(&mut a, "laser.repeat_marking", Value::Null).unwrap();
    assert!(a.machine.ext.galvo_lens_open && a.machine.ext.galvo_repeat_open);
}

#[test]
fn the_gweike_window_says_direct_control_waits_for_a_capture() {
    let ctx = egui::Context::default();
    let mut a = DarkBurn::new(Prefs::default());
    add(&mut a, DeviceType::GweikeCardSimulator);
    run(&mut a, "shape.add_rect", json!({ "x": 100, "y": 100, "width": 200, "height": 120, "layer": 0 })).unwrap();
    run(&mut a, "gweike.open", Value::Null).unwrap();
    let t = draw(&ctx, &mut a);
    assert!(t.iter().any(|x| x.contains("read-only capture")), "{t:?}");
    run(&mut a, "gweike.simulate", Value::Null).unwrap();
    // The window's body scrolls: on a screen tall enough for all of it (the card keeps clear of
    // the main window's resize border, so at 1000 points the result is just under the fold).
    let t = draw_on(&ctx, &mut a, egui::vec2(1400.0, 1200.0));
    assert!(t.iter().any(|x| x.starts_with("Simulated card:")), "{t:?}");
}
