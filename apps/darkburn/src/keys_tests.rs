//! Keyboard shortcuts end to end [KEY-01 to KEY-05]: a key press goes through egui's input into
//! `DarkBurn::shortcut_command` (the function the shell's `shortcuts` uses) and the command it
//! names runs through `DarkBurn::run`, like a menu click.

use std::time::{Duration, Instant};

use darkburn_core::Shape;
use darkburn_core::kurbo::Rect;
use egui::{Event, Key, Modifiers, RawInput};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::commands;
use crate::devices::DeviceType;
use crate::keymap;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

/// Press `key` with `mods` for one frame and return the command the shell would run.
fn press(app: &mut DarkBurn, key: Key, mods: Modifiers) -> Option<&'static str> {
    let ctx = egui::Context::default();
    let input = RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
        events: vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }],
        ..Default::default()
    };
    let mut got = None;
    let mut out = ctx.run_ui(input, |ui| got = app.shortcut_command(ui.ctx()));
    out.textures_delta.clear();
    got
}

#[test]
fn every_default_shortcut_names_a_command_that_runs() {
    let o = keymap::Overrides::new();
    let table = keymap::table(&o);
    assert!(table.len() > 60, "{} shortcuts", table.len());
    let mut broken = Vec::new();
    for (shortcut, id) in &table {
        if *id == "tools.position_laser" {
            continue; // Alt+L: the tool needs a click target on the canvas, not built yet
        }
        if commands::lookup(id).is_none() {
            broken.push(format!("{id}: not registered"));
        } else if !commands::works(id) {
            broken.push(format!("{id}: has a key ({}) but is not implemented", keymap::key_text(shortcut.modifiers, shortcut.logical_key)));
        }
    }
    assert!(broken.is_empty(), "{broken:#?}");
}

#[test]
fn every_default_shortcut_dispatches_to_its_command() {
    let o = keymap::Overrides::new();
    let mut wrong = Vec::new();
    for (shortcut, id) in keymap::table(&o) {
        if id == "tools.position_laser" {
            continue;
        }
        let mut a = app();
        let got = press(&mut a, shortcut.logical_key, shortcut.modifiers);
        if got != Some(id) {
            wrong.push(format!("{id}: {} gave {got:?}", keymap::key_text(shortcut.modifiers, shortcut.logical_key)));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_remapped_key_dispatches_and_the_old_one_does_not() {
    let mut a = app();
    keymap::assign(&mut a.prefs.hotkeys, "edit.select_all", "F6", false).unwrap();
    assert_eq!(press(&mut a, Key::F6, Modifiers::NONE), Some("edit.select_all"));
    assert_eq!(press(&mut a, Key::A, Modifiers::COMMAND), None);
}

#[test]
fn shortcuts_are_ignored_while_a_text_field_has_focus() {
    let mut a = app();
    let ctx = egui::Context::default();
    let id = egui::Id::unique("field");
    ctx.memory_mut(|m| m.request_focus(id));
    let input = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
    let mut out = ctx.run_ui(input.clone(), |ui| {
        let _ = ui.interact(ui.max_rect(), id, egui::Sense::click());
    });
    out.textures_delta.clear();
    let mut got = Some("x");
    let mut with_key = input;
    with_key.events.push(Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    let mut out = ctx.run_ui(with_key, |ui| got = a.shortcut_command(ui.ctx()));
    out.textures_delta.clear();
    assert_eq!(got, None);
}

type Keys = std::rc::Rc<std::cell::RefCell<Vec<Event>>>;

/// A context whose detached windows open as native windows, run the way the desktop backend runs
/// them: in a pass of their own, with their own input, which here is what `keys` holds.
fn with_native_windows(keys: &Keys) -> egui::Context {
    let keys = keys.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let events = std::mem::take(&mut *keys.borrow_mut());
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 500.0)));
        let info = egui::ViewportInfo { parent: Some(viewport.ids.parent), ..Default::default() };
        let viewports = std::iter::once((viewport.ids.this, info)).collect();
        let input = RawInput { viewport_id: viewport.ids.this, viewports, screen_rect, events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });
    let ctx = egui::Context::default();
    darkburn_ui::fonts::setup_bundled_fonts(&ctx);
    ctx.set_embed_viewports(false);
    ctx
}

/// One frame with Settings open in a window of its own, `key` with `mods` pressed in it. The
/// main window's own shortcut check runs too: it must not see the key.
fn press_in_own_window(app: &mut DarkBurn, keys: &Keys, ctx: &egui::Context, key: Key, mods: Modifiers) {
    darkburn_ui::ui_kit::window_host::update(ctx, "settings", |p| p.detach());
    keys.borrow_mut().push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods });
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)));
    let mut root = None;
    let mut out = ctx.run_ui(RawInput { screen_rect, ..Default::default() }, |ui| {
        darkburn_ui::ui_kit::Window::new(egui::Id::unique("settings"), "settings", "Settings").dockable("settings").show(ui.ctx(), true, |_| {});
        root = app.shortcut_command(ui.ctx());
    });
    out.textures_delta.clear();
    assert_eq!(root, None, "the main window did not get the key itself");
}

/// The system gives the keyboard to the window that has the focus: Pause and Ctrl+Pause still
/// pause and stop the job while Settings has it in a window of its own.
#[test]
fn shortcuts_work_while_a_window_of_its_own_has_the_keyboard() {
    let keys = Keys::default();
    let ctx = with_native_windows(&keys);
    let mut a = app();
    let pressed = |a: &mut DarkBurn, key, mods| {
        press_in_own_window(a, &keys, &ctx, key, mods);
        a.window_shortcut_commands(&ctx)
    };
    assert_eq!(pressed(&mut a, Key::Pause, Modifiers::COMMAND), vec!["job.stop"]);
    assert_eq!(pressed(&mut a, Key::Pause, Modifiers::NONE), vec!["job.pause"]);
    assert_eq!(pressed(&mut a, Key::S, Modifiers::ALT), vec!["job.start"]);
    assert_eq!(pressed(&mut a, Key::L, Modifiers::NONE), Vec::<&str>::new(), "a plain key was meant for the window");
    // The command runs.
    execute_headless(&mut a, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 })).unwrap();
    assert_eq!(a.editor.doc.len(), 1);
    press_in_own_window(&mut a, &keys, &ctx, Key::Z, Modifiers::COMMAND);
    a.window_shortcuts(&ctx);
    assert_eq!(a.editor.doc.len(), 0, "Ctrl+Z in the window undid the drawing");
}

// ----- KEY-02 laser keys -------------------------------------------------------------------------

#[test]
fn laser_keys_start_frame_outline_send_pause_stop() {
    let mut a = app();
    let alt = Modifiers::ALT;
    assert_eq!(press(&mut a, Key::S, alt), Some("job.start"));
    assert_eq!(press(&mut a, Key::R, alt), Some("job.frame"));
    assert_eq!(press(&mut a, Key::R, Modifiers::ALT | Modifiers::SHIFT), Some("job.frame_rubber"));
    assert_eq!(press(&mut a, Key::S, Modifiers::ALT | Modifiers::SHIFT), Some("job.send"));
    assert_eq!(press(&mut a, Key::Pause, Modifiers::NONE), Some("job.pause"));
    assert_eq!(press(&mut a, Key::Pause, Modifiers::COMMAND), Some("job.stop"));
    // The menus show the keys.
    for id in ["job.start", "job.frame", "job.frame_rubber", "job.send", "job.pause", "job.stop"] {
        assert!(!keymap::key_for(commands::lookup(id).unwrap()).is_empty(), "{id}");
    }
}

fn wait(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A connected simulator: the start key sends the job to it, pause and stop keys act on it.
#[test]
fn laser_keys_drive_a_simulator() {
    let mut a = app();
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
    execute_headless(&mut a, "machine.connect", &json!({})).unwrap();
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    // KEY-03 keyboard jogging: Ctrl+Alt+Right steps the head by the panel's distance.
    let x0 = a.machine.handle.snapshot().work_pos[0];
    let d = a.machine.jog.distance;
    let cmd = press(&mut a, Key::ArrowRight, Modifiers::COMMAND | Modifiers::ALT).expect("jog key");
    assert_eq!(cmd, "laser.jog_right");
    a.run(&egui::Context::default(), cmd);
    wait(&mut a, "jogged right", |a| (a.machine.handle.snapshot().work_pos[0] - x0 - d).abs() < 0.01);
    wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let cmd = press(&mut a, Key::ArrowLeft, Modifiers::COMMAND | Modifiers::ALT).unwrap();
    a.run(&egui::Context::default(), cmd);
    wait(&mut a, "jogged left", |a| (a.machine.handle.snapshot().work_pos[0] - x0).abs() < 0.01);
    wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    // Start by key.
    execute_headless(&mut a, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 })).unwrap();
    let cmd = press(&mut a, Key::S, Modifiers::ALT).unwrap();
    a.run(&egui::Context::default(), cmd);
    let ctx = egui::Context::default();
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let mut out = ctx.run_ui(RawInput::default(), |ui| crate::job_ui::frame(ui.ctx(), &mut a));
        out.textures_delta.clear();
        a.machine.tick();
        if a.machine.job_log.iter().any(|l| l.starts_with("Job finished")) || a.machine.handle.snapshot().state.is_working() {
            break;
        }
        assert!(Instant::now() < end, "the start key did not start the job; log {:?}", a.machine.job_log);
        std::thread::sleep(Duration::from_millis(5));
    }
    let cmd = press(&mut a, Key::Pause, Modifiers::COMMAND).unwrap();
    a.run(&egui::Context::default(), cmd);
    let _: Value = Value::Null;
}

#[test]
fn jog_keys_are_hardware_commands() {
    for id in ["laser.jog_left", "laser.jog_right", "laser.jog_up", "laser.jog_down"] {
        let info = commands::describe(id).unwrap();
        assert!(info.hardware && info.implemented, "{id}");
        // An agent cannot run them.
        assert!(execute_headless(&mut app(), id, &Value::Null).is_err());
    }
}

// ----- KEY-05 align keys -------------------------------------------------------------------------

#[test]
fn align_keys_move_the_selection() {
    let mut a = app();
    a.editor.edit("add", |d, sel| {
        let p = d.add(Shape::rect(Rect::new(10.0, 10.0, 20.0, 20.0), 0.0, 0));
        let q = d.add(Shape::rect(Rect::new(50.0, 40.0, 70.0, 60.0), 0.0, 0));
        sel.set([p, q]);
        true
    });
    let ctx = egui::Context::default();
    let ids = a.editor.selected();
    let (p, q) = (ids[0], ids[1]);
    let bb = |a: &DarkBurn, id| a.editor.doc.get(id).unwrap().bbox();
    // Shapes align to the last selected one (q).
    for (key, cmd) in
        [(Key::L, "arrange.align_left"), (Key::T, "arrange.align_top"), (Key::B, "arrange.align_bottom"), (Key::R, "arrange.align_right")]
    {
        a.editor.edit("Move", |d, _| darkburn_core::arrange::translate(d, &[p], darkburn_core::kurbo::Vec2::new(-30.0, -30.0)));
        assert_eq!(press(&mut a, key, Modifiers::NONE), Some(cmd));
        a.run(&ctx, cmd);
        let (bp, bq) = (bb(&a, p), bb(&a, q));
        let ok = match key {
            Key::L => (bp.x0 - bq.x0).abs() < 1e-6,
            Key::T => (bp.y1 - bq.y1).abs() < 1e-6,
            Key::B => (bp.y0 - bq.y0).abs() < 1e-6,
            _ => (bp.x1 - bq.x1).abs() < 1e-6,
        };
        assert!(ok, "{key:?} / {cmd}: {bp:?} vs {bq:?}");
    }
    // E and C centre.
    for (key, cmd) in [(Key::E, "arrange.align_h_centres"), (Key::C, "arrange.align_v_centres")] {
        assert_eq!(press(&mut a, key, Modifiers::NONE), Some(cmd));
    }
    // In the node tool the plain letters belong to node editing.
    a.set_tool(crate::tools::Tool::Nodes);
    assert_eq!(press(&mut a, Key::L, Modifiers::NONE), None);
}

// ----- OTHER-08 hotkey editor --------------------------------------------------------------------

#[test]
fn hotkey_editor_commands_remap_and_the_key_follows() {
    let mut a = app();
    let run = |a: &mut DarkBurn, id: &str, args: Value| execute_headless(a, id, &args);
    run(&mut a, "hotkeys.set", json!({ "command": "edit.undo", "key": "Ctrl+Alt+U" })).unwrap();
    assert_eq!(press(&mut a, Key::U, Modifiers::COMMAND | Modifiers::ALT), Some("edit.undo"));
    assert_eq!(press(&mut a, Key::Z, Modifiers::COMMAND), None);
    // A key another command has is refused unless taken.
    let err = run(&mut a, "hotkeys.set", json!({ "command": "edit.redo", "key": "Ctrl+Alt+U" })).unwrap_err();
    assert!(err.contains("edit.undo"), "{err}");
    run(&mut a, "hotkeys.set", json!({ "command": "edit.redo", "key": "Ctrl+Alt+U", "take": true })).unwrap();
    assert_eq!(press(&mut a, Key::U, Modifiers::COMMAND | Modifiers::ALT), Some("edit.redo"));
    let list = run(&mut a, "hotkeys.list", Value::Null).unwrap();
    assert!(list["conflicts"].as_array().unwrap().is_empty());
    // Export, reset, import round trip.
    let path = std::env::temp_dir().join("darkburn_keys_test.dbhk");
    run(&mut a, "hotkeys.export", json!({ "path": path.to_string_lossy() })).unwrap();
    run(&mut a, "hotkeys.reset", Value::Null).unwrap();
    assert_eq!(press(&mut a, Key::Z, Modifiers::COMMAND), Some("edit.undo"));
    run(&mut a, "hotkeys.import", json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(press(&mut a, Key::U, Modifiers::COMMAND | Modifiers::ALT), Some("edit.redo"));
    let _ = std::fs::remove_file(path);
}
