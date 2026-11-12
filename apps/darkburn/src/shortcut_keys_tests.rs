//! Second keys, Z and diagonal jog keys, number pad jogging and Y/N answers [KEY-01, KEY-03]:
//! the keys go through `DarkBurn::shortcut_command` and the commands through `DarkBurn::run`, as
//! a real key press does; the harness tests press them in the drawn window.

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, RawInput};
use serde_json::json;

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::keymap;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use crate::ui_harness::Gui;

const NOT_CONNECTED: &str = "The laser is not connected";
const BUSY: &str = "The laser is busy";
const NO_Z: &str = "no Z axis";

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn screen() -> Option<egui::Rect> {
    Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)))
}

/// Press `key` (on the keyboard at `physical`) with `mods` for one frame and return the command
/// the shell would run. With `typing`, a text field has the keyboard.
fn press_at(app: &mut DarkBurn, key: Key, physical: Option<Key>, mods: Modifiers, typing: bool) -> Option<&'static str> {
    let ctx = egui::Context::default();
    let field = egui::Id::unique("shortcut_keys_field");
    let mut text = String::new();
    if typing {
        let mut out = ctx.run_ui(RawInput { screen_rect: screen(), ..Default::default() }, |ui| {
            ui.add(egui::TextEdit::singleline(&mut text).id(field)).request_focus();
        });
        out.textures_delta.clear();
    }
    let input = RawInput {
        screen_rect: screen(),
        events: vec![Event::Key { key, physical_key: physical, pressed: true, repeat: false, modifiers: mods }],
        ..Default::default()
    };
    let mut got = None;
    let mut out = ctx.run_ui(input, |ui| {
        if typing {
            ui.add(egui::TextEdit::singleline(&mut text).id(field));
            assert!(ui.ctx().text_edit_focused(), "the field has the keyboard");
        }
        got = app.shortcut_command(ui.ctx());
    });
    out.textures_delta.clear();
    got
}

fn press(app: &mut DarkBurn, key: Key, mods: Modifiers) -> Option<&'static str> {
    press_at(app, key, None, mods, false)
}

/// A number pad key with NumLock off: `key` is what it means (an arrow, Home, Page Up) and
/// `pad` the digit printed on it.
fn pad(app: &mut DarkBurn, key: Key, pad: Key) -> Option<&'static str> {
    press_at(app, key, Some(pad), Modifiers::NONE, false)
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

fn state(a: &DarkBurn) -> LaserState {
    a.machine.handle.snapshot().state
}

fn pos(a: &DarkBurn) -> [f64; 4] {
    a.machine.handle.snapshot().work_pos
}

fn connect_sim(a: &mut DarkBurn) {
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    execute_headless(a, "device.select", &json!({ "id": id })).unwrap();
    execute_headless(a, "machine.connect", &json!({})).unwrap();
    wait(a, "connected and ready", |a| state(a) == LaserState::Ready);
}

fn set_z(a: &mut DarkBurn, on: bool) {
    let id = a.machine.active().id;
    a.machine.devices.devices.iter_mut().find(|d| d.id == id).unwrap().settings.z_enabled = on;
}

fn message(a: &DarkBurn) -> String {
    a.machine.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
}

/// Run a jog key the way the shell does and say what the laser panel then shows.
fn run_key(a: &mut DarkBurn, cmd: &str) -> String {
    a.machine.message = None;
    a.run(&egui::Context::default(), cmd);
    message(a)
}

// ----- second keys ------------------------------------------------------------------------------

#[test]
fn f2_starts_the_job_like_alt_s() {
    let mut a = app();
    assert_eq!(press(&mut a, Key::F2, Modifiers::NONE), Some("job.start"));
    assert_eq!(press(&mut a, Key::S, Modifiers::ALT), Some("job.start"), "the first key still works");
    // Over a text field F2 is not a start key.
    assert_eq!(press_at(&mut a, Key::F2, None, Modifiers::NONE, true), None);
    // Moved elsewhere, F2 stops starting the job and Alt+S keeps doing it.
    keymap::assign(&mut a.prefs.hotkeys, &keymap::second_of("job.start"), "F9", false).unwrap();
    assert_eq!(press(&mut a, Key::F2, Modifiers::NONE), None);
    assert_eq!(press(&mut a, Key::F9, Modifiers::NONE), Some("job.start"));
    assert_eq!(press(&mut a, Key::S, Modifiers::ALT), Some("job.start"));
}

/// F2 is the same command as the Start button, so the same gate: no laser, no job.
#[test]
fn f2_without_a_laser_is_refused_like_the_start_button() {
    let mut a = app();
    execute_headless(&mut a, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 })).unwrap();
    let by_key = {
        let cmd = press(&mut a, Key::F2, Modifiers::NONE).unwrap();
        a.run(&egui::Context::default(), cmd);
        (state(&a), a.machine.handle.snapshot().job.is_some())
    };
    assert_eq!(by_key, (LaserState::Disconnected, false), "nothing was sent anywhere");
}

// ----- Z and diagonal jog keys ------------------------------------------------------------------

#[test]
fn z_and_diagonal_jog_keys_dispatch() {
    let mut a = app();
    let ca = Modifiers::COMMAND | Modifiers::ALT;
    for (key, cmd) in [
        (Key::Home, "laser.jog_up_left"),
        (Key::PageUp, "laser.jog_up_right"),
        (Key::End, "laser.jog_down_left"),
        (Key::PageDown, "laser.jog_down_right"),
        (Key::CloseBracket, "laser.jog_z_up"),
        (Key::OpenBracket, "laser.jog_z_down"),
    ] {
        assert_eq!(press(&mut a, key, ca), Some(cmd), "{key:?}");
        assert_eq!(press_at(&mut a, key, None, ca, true), None, "{key:?} while typing");
    }
    // Without Ctrl+Alt those keys keep their ordinary meaning.
    assert_eq!(press(&mut a, Key::OpenBracket, Modifiers::NONE), None);
}

#[test]
fn jog_keys_are_refused_without_a_laser() {
    let mut a = app();
    for c in crate::key_cmds::COMMANDS {
        let said = run_key(&mut a, c.id);
        assert!(said.contains(NOT_CONNECTED), "{}: {said:?}", c.id);
    }
    assert_eq!(state(&a), LaserState::Disconnected);
}

#[test]
fn diagonal_keys_move_both_axes_and_z_keys_need_a_z_axis() {
    let mut a = app();
    connect_sim(&mut a);
    let d = a.machine.jog.distance;
    // Diagonals: both axes by the step.
    let p0 = pos(&a);
    assert_eq!(run_key(&mut a, "laser.jog_up_right"), "");
    wait(&mut a, "diagonal step", |a| {
        let p = pos(a);
        ((p[0] - p0[0]).abs() - d).abs() < 0.01 && ((p[1] - p0[1]).abs() - d).abs() < 0.01
    });
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    let p1 = pos(&a);
    run_key(&mut a, "laser.jog_down_left");
    wait(&mut a, "back again", |a| (pos(a)[0] - p0[0]).abs() < 0.01 && (pos(a)[1] - p0[1]).abs() < 0.01);
    assert!((p1[0] - p0[0]) * (p1[1] - p0[1]) != 0.0);
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    // Z: refused as long as the Move panel has no Z buttons.
    set_z(&mut a, false);
    assert!(!crate::key_cmds::has_z(&a.machine));
    let z0 = pos(&a)[2];
    let said = run_key(&mut a, "laser.jog_z_up");
    assert!(said.contains(NO_Z), "{said:?}");
    std::thread::sleep(Duration::from_millis(100));
    a.machine.tick();
    assert_eq!(pos(&a)[2], z0, "a refused key did not move Z");
    // With Z switched on, Z up and Z down step it like the panel's buttons.
    set_z(&mut a, true);
    assert_eq!(run_key(&mut a, "laser.jog_z_up"), "");
    wait(&mut a, "Z up", |a| (pos(a)[2] - z0 - d).abs() < 0.01);
    wait(&mut a, "idle", |a| state(a) == LaserState::Ready);
    run_key(&mut a, "laser.jog_z_down");
    wait(&mut a, "Z down", |a| (pos(a)[2] - z0).abs() < 0.01);
}

/// While a job runs (and while it is paused) the jog keys are refused, as the panel's buttons
/// are disabled then.
#[test]
fn jog_keys_are_refused_while_a_job_runs() {
    let mut a = app();
    connect_sim(&mut a);
    set_z(&mut a, true);
    let mut j = crate::machine_api::JobRequest::gcode(
        std::iter::once("G21 G90".to_owned())
            .chain(std::iter::once("M4 S100".to_owned()))
            .chain((0..60).map(|i| format!("G1 X{} Y10 F300", 10 + (i % 2) * 30)))
            .collect(),
    );
    j.name = "slow".into();
    a.machine.begin_start(j).unwrap();
    wait(&mut a, "busy", |a| state(a) == LaserState::Busy);
    for c in crate::key_cmds::COMMANDS {
        let said = run_key(&mut a, c.id);
        assert!(said.contains(BUSY), "{}: {said:?}", c.id);
    }
    execute_headless(&mut a, "machine.pause", &json!({})).unwrap();
    wait(&mut a, "paused", |a| state(a) == LaserState::Paused);
    let said = run_key(&mut a, "laser.jog_left");
    assert!(said.contains(BUSY), "paused: {said:?}");
    a.machine.stop_and_close();
    wait(&mut a, "stopped", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
}

// ----- number pad (NumLock off) -----------------------------------------------------------------

#[test]
fn number_pad_keys_jog_only_with_a_laser_and_never_from_the_digit_row() {
    let mut a = app();
    // No laser: the pad's arrows stay arrows (they nudge the selection, as before).
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), None);
    connect_sim(&mut a);
    for (key, digit, cmd) in [
        (Key::ArrowUp, Key::Num8, "laser.jog_up"),
        (Key::ArrowDown, Key::Num2, "laser.jog_down"),
        (Key::ArrowLeft, Key::Num4, "laser.jog_left"),
        (Key::ArrowRight, Key::Num6, "laser.jog_right"),
        (Key::Home, Key::Num7, "laser.jog_up_left"),
        (Key::PageUp, Key::Num9, "laser.jog_up_right"),
        (Key::End, Key::Num1, "laser.jog_down_left"),
        (Key::PageDown, Key::Num3, "laser.jog_down_right"),
    ] {
        assert_eq!(pad(&mut a, key, digit), Some(cmd), "pad {digit:?}");
        // The top-row digit (or NumLock on: the same key to egui) does not jog.
        assert_eq!(pad(&mut a, digit, digit), None, "digit {digit:?}");
        // The ordinary arrow key is not a pad key.
        assert_eq!(pad(&mut a, key, key), None, "{key:?}");
        // Not while a text field has the keyboard, and not with Ctrl or Alt held.
        assert_eq!(press_at(&mut a, key, Some(digit), Modifiers::NONE, true), None);
        assert_ne!(press_at(&mut a, key, Some(digit), Modifiers::ALT, false), Some(cmd));
    }
    // A pad key jogs through the same command, so by the panel's step.
    let (x0, d) = (pos(&a)[0], a.machine.jog.distance);
    let cmd = pad(&mut a, Key::ArrowRight, Key::Num6).unwrap();
    assert_eq!(run_key(&mut a, cmd), "");
    wait(&mut a, "pad 6 jogged", |a| ((pos(a)[0] - x0).abs() - d).abs() < 0.01);
}

#[test]
fn a_number_pad_key_can_be_moved_and_removed() {
    let mut a = app();
    connect_sim(&mut a);
    let up2 = keymap::second_of("laser.jog_up");
    keymap::assign(&mut a.prefs.hotkeys, &up2, "", false).unwrap();
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), None, "removed");
    keymap::assign(&mut a.prefs.hotkeys, &up2, "Numpad5", false).unwrap();
    assert_eq!(pad(&mut a, Key::Num5, Key::Num5), None, "NumLock on is the digit row");
    // Pad 5 with NumLock off sends no key egui knows; a layout that sends one is matched by place.
    assert_eq!(pad(&mut a, Key::Space, Key::Num5), Some("laser.jog_up"));
    keymap::reset(&mut a.prefs.hotkeys, "laser.jog_up");
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), Some("laser.jog_up"));
}

// ----- in the window ----------------------------------------------------------------------------

fn gui_connect(g: &mut Gui) {
    {
        let mut a = g.app();
        let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
        execute_headless(&mut a, "device.select", &json!({ "id": id })).unwrap();
        execute_headless(&mut a, "machine.connect", &json!({})).unwrap();
    }
    gui_wait(g, "connected", |a| state(a) == LaserState::Ready);
}

fn gui_wait(g: &mut Gui, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        g.app().machine.tick();
        if f(&g.app()) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        g.frames(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ui_jog_keys_are_refused_without_a_laser_and_jog_with_one() {
    let mut g = Gui::standard();
    let ca = Modifiers::COMMAND | Modifiers::ALT;
    g.key(Key::PageUp, ca);
    let said = message(&g.app());
    assert!(said.contains(NOT_CONNECTED), "{said:?}");
    g.screenshot("shortcut_keys_jog_refused");
    // The pad arrow without a laser is just an arrow: no complaint.
    g.app().machine.message = None;
    g.key_at(Key::ArrowUp, Key::Num8);
    assert_eq!(message(&g.app()), "");
    gui_connect(&mut g);
    let p0 = pos(&g.app());
    let d = g.app().machine.jog.distance;
    g.key(Key::PageUp, ca);
    gui_wait(&mut g, "diagonal by key", |a| ((pos(a)[0] - p0[0]).abs() - d).abs() < 0.01 && ((pos(a)[1] - p0[1]).abs() - d).abs() < 0.01);
    gui_wait(&mut g, "idle", |a| state(a) == LaserState::Ready);
    let p1 = pos(&g.app());
    g.key_at(Key::ArrowDown, Key::Num2);
    gui_wait(&mut g, "pad 2 by key", |a| ((pos(a)[1] - p1[1]).abs() - d).abs() < 0.01);
    gui_wait(&mut g, "idle", |a| state(a) == LaserState::Ready);
    // Z on a machine without Z: refused, and said so.
    g.app().machine.message = None;
    g.key(Key::CloseBracket, ca);
    let said = message(&g.app());
    assert!(said.contains(NO_Z), "{said:?}");
}

#[test]
fn ui_jog_keys_type_into_a_focused_field() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    let area = g.app().prefs.panel("console").area;
    g.app().prefs.active_tab.insert(area, "console".to_string());
    g.frames(3);
    g.click_a11y(Some("TextInput"), "Console command");
    assert!(g.ctx().text_edit_focused(), "the console field has the keyboard");
    let p0 = pos(&g.app());
    g.app().machine.message = None;
    g.key(Key::PageUp, Modifiers::COMMAND | Modifiers::ALT);
    g.key_at(Key::ArrowUp, Key::Num8);
    g.frames(5);
    g.app().machine.tick();
    assert_eq!(message(&g.app()), "");
    assert_eq!(pos(&g.app()), p0, "nothing jogged while typing");
    assert_eq!(state(&g.app()), LaserState::Ready);
}

#[test]
fn ui_jog_keys_are_refused_while_a_job_runs() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    {
        let mut a = g.app();
        let mut j = crate::machine_api::JobRequest::gcode(
            std::iter::once("G21 G90".to_owned())
                .chain(std::iter::once("M4 S100".to_owned()))
                .chain((0..60).map(|i| format!("G1 X{} Y10 F300", 10 + (i % 2) * 30)))
                .collect(),
        );
        j.name = "slow".into();
        a.machine.begin_start(j).unwrap();
    }
    gui_wait(&mut g, "busy", |a| state(a) == LaserState::Busy);
    g.app().machine.message = None;
    g.key_at(Key::ArrowLeft, Key::Num4);
    let said = message(&g.app());
    assert!(said.contains(BUSY), "{said:?}");
    g.app().machine.message = None;
    g.key(Key::Home, Modifiers::COMMAND | Modifiers::ALT);
    let said = message(&g.app());
    assert!(said.contains(BUSY), "{said:?}");
    g.app().machine.stop_and_close();
    gui_wait(&mut g, "stopped", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
}

#[test]
fn ui_hotkey_editor_shows_and_edits_the_second_key() {
    let mut g = Gui::standard();
    g.run_command("file.hotkeys");
    assert!(g.has_text("Second key"), "{:?}", g.window_titles());
    g.app().extras.hotkeys.filter = "jog".into();
    g.frames(3);
    for key in ["Numpad8", "Numpad7", "Ctrl+Alt+Home", "Ctrl+Alt+]"] {
        assert!(g.has_text(key), "{key} is shown");
    }
    g.screenshot("shortcut_keys_hotkey_editor");
    g.app().extras.hotkeys.filter = "job.start".into();
    g.frames(3);
    assert!(g.has_text("F2"), "F2 is shown as the second start key");
    assert!(g.has_text("Alt+S"), "next to the first");
    // Right-click removes only the second key.
    let at = g.find_text("F2").unwrap();
    g.right_click_pos(at.center());
    let o = g.app().prefs.hotkeys.clone();
    assert_eq!(keymap::key_in(&o, &keymap::second_of("job.start"), "F2"), "");
    assert_eq!(keymap::key_in(&o, "job.start", "Alt+S"), "Alt+S");
    // Click the empty second cell and press a key: it becomes the second key.
    g.click_pos(at.center());
    g.key(Key::F8, Modifiers::NONE);
    let o = g.app().prefs.hotkeys.clone();
    assert_eq!(keymap::key_in(&o, &keymap::second_of("job.start"), "F2"), "F8");
    g.close_dialogs();
    let mut a = g.app();
    assert_eq!(press(&mut a, Key::F8, Modifiers::NONE), Some("job.start"));
}

// ----- Y and N ----------------------------------------------------------------------------------

#[test]
fn ui_y_and_n_answer_the_restore_question() {
    // Y restores (the copy here is missing, so it says it could not).
    let mut g = Gui::standard();
    let missing = g.temp("no-such-autosave.dbp");
    g.app().files.restore = Some(missing.clone());
    g.frames(3);
    assert!(g.has_text("Restore auto-saved project?"));
    g.screenshot("shortcut_keys_restore_question");
    g.key(Key::Y, Modifiers::NONE);
    assert!(g.app().files.restore.is_none(), "Y answered");
    assert!(g.app().files.message.is_some(), "Y chose Restore");
    // N discards.
    let mut g = Gui::standard();
    g.app().files.restore = Some(missing);
    g.frames(3);
    g.key(Key::N, Modifiers::NONE);
    assert!(g.app().files.restore.is_none(), "N answered");
    assert!(g.app().files.message.is_none(), "N chose Discard");
}

/// A box that would act on the machine takes N for no, and Y never says yes to it.
#[test]
fn ui_y_does_not_confirm_a_machine_action_and_n_refuses_it() {
    let mut g = Gui::standard();
    g.app().machine.dlg.confirm_close = true;
    g.frames(3);
    assert!(g.has_text("A job is running"));
    g.key(Key::Y, Modifiers::NONE);
    assert!(g.app().machine.dlg.confirm_close, "Y did not stop and close");
    assert!(!g.app().machine.close_ok);
    g.screenshot("shortcut_keys_close_question");
    g.key(Key::N, Modifiers::NONE);
    assert!(!g.app().machine.dlg.confirm_close, "N kept the job running");
    assert!(!g.app().machine.close_ok);
}
