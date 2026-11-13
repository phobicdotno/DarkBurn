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
        // The ordinary key (an arrow, Page Up) is not a pad key: it keeps what it did before.
        assert_ne!(pad(&mut a, key, key), Some(cmd), "{key:?}");
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
    // Pad 5 cannot be told from the digit 5, so it cannot be a pad key.
    assert_eq!(keymap::assign(&mut a.prefs.hotkeys, &up2, "Numpad5", false), Err(keymap::Refused::BadKey));
    keymap::assign(&mut a.prefs.hotkeys, &up2, "Numpad0", false).unwrap();
    assert_eq!(pad(&mut a, Key::Num0, Key::Num0), None, "NumLock on is the digit row");
    assert_eq!(pad(&mut a, Key::Insert, Key::Num0), Some("laser.jog_up"), "pad 0 sends Insert with NumLock off");
    keymap::reset(&mut a.prefs.hotkeys, "laser.jog_up");
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), Some("laser.jog_up"));
}

/// Keys on the digit row that type punctuation also have a digit as their physical key; they
/// are not number pad keys and never jog.
#[test]
fn shifted_digits_and_azerty_punctuation_do_not_jog() {
    let mut a = app();
    connect_sim(&mut a);
    let p0 = pos(&a);
    for (key, digit, mods, what) in [
        (Key::Exclamationmark, Key::Num1, Modifiers::SHIFT, "Shift+1 on a US or German keyboard"),
        (Key::Slash, Key::Num7, Modifiers::SHIFT, "Shift+7 on a Nordic or German keyboard"),
        (Key::Minus, Key::Num6, Modifiers::NONE, "the AZERTY 6 key"),
        (Key::Quote, Key::Num4, Modifiers::NONE, "the AZERTY 4 key"),
    ] {
        let got = press_at(&mut a, key, Some(digit), mods, false);
        assert!(!got.is_some_and(|c| c.starts_with("laser.")), "{what}: {got:?}");
    }
    a.machine.message = None;
    std::thread::sleep(Duration::from_millis(100));
    a.machine.tick();
    assert_eq!(pos(&a), p0, "nothing moved");
    assert_eq!(message(&a), "", "and nothing was refused either");
}

/// The node tool owns every plain key: the pad's arrows (NumLock off) nudge its nodes, as the
/// arrow keys do, and do not jog the laser.
#[test]
fn the_number_pad_leaves_the_node_tool_its_arrows() {
    let mut a = app();
    connect_sim(&mut a);
    a.set_tool(crate::tools::Tool::Nodes);
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), None);
    assert_eq!(pad(&mut a, Key::Home, Key::Num7), None);
    a.set_tool(crate::tools::Tool::Select);
    assert_eq!(pad(&mut a, Key::ArrowUp, Key::Num8), Some("laser.jog_up"));
}

/// F2 starts a job on a connected laser, the same as Alt+S and the Start button.
#[test]
fn f2_starts_a_job_on_the_simulator() {
    let mut a = app();
    connect_sim(&mut a);
    execute_headless(&mut a, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 })).unwrap();
    let cmd = press(&mut a, Key::F2, Modifiers::NONE).unwrap();
    assert_eq!(cmd, "job.start");
    a.run(&egui::Context::default(), cmd);
    // A key starts the job the way a button does: the job is built in the background and a frame
    // hands it to the laser. Without a window, wait for the build and hand it over here.
    let end = Instant::now() + Duration::from_secs(20);
    let job = loop {
        let (task, _) = a.job.pending.as_mut().expect("F2 builds the job");
        if let Some(done) = task.poll().cloned() {
            break done.expect("the job builds");
        }
        assert!(Instant::now() < end, "timed out building the job");
        std::thread::sleep(Duration::from_millis(5));
    };
    let (_, then) = a.job.pending.take().unwrap();
    crate::job_cmds::finish(&mut a, job, then);
    assert!(a.job.message.is_none(), "{:?}", a.job.message.as_ref().map(|m| &m.text));
    wait(&mut a, "the job under way", |a| state(a) == LaserState::Busy || a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    a.machine.stop_and_close();
    wait(&mut a, "stopped", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
}

/// Preferences saved before F2 and the Ctrl+Alt jog keys were defaults may use those keys for
/// something else: the user's keys keep working and neither starts a job nor jogs.
#[test]
fn keys_the_user_set_up_before_beat_the_new_defaults() {
    let prefs = Prefs::from_json(r#"{"hotkeys":{"view.zoom_page":"F2","view.pan":"Ctrl+Alt+Home"}}"#);
    assert_eq!(prefs.hotkeys.get("view.zoom_page").map(String::as_str), Some("F2"), "kept when loaded");
    let mut a = DarkBurn::new(prefs);
    connect_sim(&mut a);
    execute_headless(&mut a, "shape.add_rect", &json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 })).unwrap();
    let f2 = press(&mut a, Key::F2, Modifiers::NONE);
    assert_ne!(f2, Some("job.start"));
    assert_ne!(press(&mut a, Key::Home, Modifiers::COMMAND | Modifiers::ALT), Some("laser.jog_up_left"));
    assert_eq!(press(&mut a, Key::S, Modifiers::ALT), Some("job.start"), "Start keeps its first key");
    // The user's F2 runs what they gave it.
    if let Some(cmd) = f2 {
        assert_eq!(cmd, "view.zoom_page");
        a.run(&egui::Context::default(), cmd);
    }
    std::thread::sleep(Duration::from_millis(100));
    a.machine.tick();
    assert_eq!(state(&a), LaserState::Ready, "no job started");
    assert!(a.machine.handle.snapshot().job.is_none());
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
    g.key_at(Key::ArrowUp, Key::Num8, Modifiers::NONE);
    assert_eq!(message(&g.app()), "");
    gui_connect(&mut g);
    let p0 = pos(&g.app());
    let d = g.app().machine.jog.distance;
    g.key(Key::PageUp, ca);
    gui_wait(&mut g, "diagonal by key", |a| ((pos(a)[0] - p0[0]).abs() - d).abs() < 0.01 && ((pos(a)[1] - p0[1]).abs() - d).abs() < 0.01);
    gui_wait(&mut g, "idle", |a| state(a) == LaserState::Ready);
    let p1 = pos(&g.app());
    g.key_at(Key::ArrowDown, Key::Num2, Modifiers::NONE);
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
    g.key_at(Key::ArrowUp, Key::Num8, Modifiers::NONE);
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
    g.key_at(Key::ArrowLeft, Key::Num4, Modifiers::NONE);
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

/// Y does not write risky settings; N says Cancel and nothing is written.
#[test]
fn ui_y_does_not_write_risky_settings_and_n_cancels() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    {
        let mut a = g.app();
        a.machine.dlg.machine_settings = true;
        a.machine.dlg.setting_edits.insert(30, "500".into());
        a.machine.dlg.message = Some("risky".into());
    }
    g.frames(3);
    assert!(g.has_text("Change protection settings?"), "{:?}", g.window_titles());
    g.key(Key::Y, Modifiers::NONE);
    assert_eq!(g.app().machine.dlg.message.as_deref(), Some("risky"), "Y did not write anyway");
    assert!(g.app().machine.dlg.setting_edits.contains_key(&30), "nothing was written");
    g.key(Key::N, Modifiers::NONE);
    assert_eq!(g.app().machine.dlg.message, None, "N cancelled");
    assert!(g.app().machine.dlg.setting_edits.contains_key(&30), "and wrote nothing");
    assert!(!g.has_text("Change protection settings?"));
}

/// Y does not restore a settings backup to the laser; N says Cancel.
#[test]
fn ui_y_does_not_restore_settings_and_n_cancels() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    {
        let mut a = g.app();
        a.machine.dlg.machine_settings = true;
        let backup = crate::machine_api::SettingsBackup::parse(crate::machine_api::Family::Grbl, "$110=1234\n").unwrap();
        a.machine.dlg.confirm_restore = Some(backup);
    }
    g.frames(3);
    assert!(g.has_text("Write these settings to the laser?"), "{:?}", g.window_titles());
    g.key(Key::Y, Modifiers::NONE);
    assert!(g.app().machine.dlg.confirm_restore.is_some(), "Y did not write the backup");
    g.key(Key::N, Modifiers::NONE);
    assert!(g.app().machine.dlg.confirm_restore.is_none(), "N cancelled");
    // The controller still has its own value.
    execute_headless(&mut g.app(), "machine.read_settings", &json!({})).unwrap();
    gui_wait(&mut g, "settings read", |a| a.machine.handle.snapshot().grbl_settings.contains_key(&110));
    let v = g.app().machine.handle.snapshot().grbl_settings.get(&110).cloned().unwrap_or_default();
    assert!(!v.starts_with("1234"), "the backup was written: {v}");
}

/// Y does not connect a real machine; N says Cancel and nothing is opened.
#[test]
fn ui_y_does_not_connect_a_real_machine_and_n_cancels() {
    use crate::devices::{ConnectionKind, Device};
    let mut g = Gui::standard();
    {
        let mut a = g.app();
        // A documentation address: nothing is ever opened, test builds refuse real links anyway.
        let id = a.machine.devices.add(Device {
            name: "Real".into(),
            connection: ConnectionKind::Tcp,
            host: "192.0.2.1".into(),
            ..Device::new(DeviceType::Grbl)
        });
        a.machine.devices.select(id);
        assert!(crate::machine_ui::run_with(&mut a, "machine.connect", &json!({})));
        assert!(a.machine.dlg.confirm_connect.is_some(), "Connect asks first");
    }
    g.frames(3);
    assert!(g.has_text("Connect to a real machine?"), "{:?}", g.window_titles());
    g.key(Key::Y, Modifiers::NONE);
    assert!(g.app().machine.dlg.confirm_connect.is_some(), "Y did not connect");
    assert!(!g.app().machine.handle.is_real_machine());
    g.screenshot("shortcut_keys_connect_question");
    g.key(Key::N, Modifiers::NONE);
    assert!(g.app().machine.dlg.confirm_connect.is_none(), "N cancelled");
    assert!(!g.app().machine.handle.is_real_machine());
    assert_eq!(state(&g.app()), LaserState::Disconnected);
}

/// F2 pressed in the window starts the job on the connected simulator.
#[test]
fn ui_f2_starts_the_job() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    g.exec("shape.add_rect", json!({ "x": 10, "y": 10, "width": 10, "height": 10, "layer": 0 }));
    g.frames(2);
    g.key(Key::F2, Modifiers::NONE);
    gui_wait(&mut g, "the job under way", |a| state(a) == LaserState::Busy || a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    g.app().machine.stop_and_close();
    gui_wait(&mut g, "stopped", |a| matches!(state(a), LaserState::Ready | LaserState::Alarm));
}

/// Shift+1 pressed in the window does not jog, and the Hotkeys editor keeps it as Shift+1's
/// own key, not as a number pad key.
#[test]
fn ui_a_shifted_digit_is_not_a_number_pad_key() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    let p0 = pos(&g.app());
    g.app().machine.message = None;
    g.key_at(Key::Exclamationmark, Key::Num1, Modifiers::SHIFT);
    g.key_at(Key::Minus, Key::Num6, Modifiers::NONE);
    g.frames(5);
    g.app().machine.tick();
    assert_eq!(pos(&g.app()), p0, "nothing jogged");
    assert_eq!(message(&g.app()), "");
    // In the Hotkeys editor: capture a key for Start's second key.
    g.run_command("file.hotkeys");
    g.app().extras.hotkeys.filter = "job.start".into();
    g.frames(3);
    let at = g.find_text("F2").unwrap();
    g.click_pos(at.center());
    g.key_at(Key::Exclamationmark, Key::Num1, Modifiers::SHIFT);
    let key = keymap::key_of(&g.app().prefs.hotkeys, &keymap::second_of("job.start"));
    assert_ne!(key, "Numpad1");
    assert!(!key.starts_with(keymap::NUMPAD), "{key}");
    g.close_dialogs();
}

/// In the node tool the pad's arrows are the node tool's, also with a laser connected.
#[test]
fn ui_the_number_pad_does_not_jog_in_the_node_tool() {
    let mut g = Gui::standard();
    gui_connect(&mut g);
    g.app().set_tool(crate::tools::Tool::Nodes);
    g.frames(2);
    let p0 = pos(&g.app());
    g.app().machine.message = None;
    g.key_at(Key::ArrowUp, Key::Num8, Modifiers::NONE);
    g.frames(5);
    g.app().machine.tick();
    assert_eq!(pos(&g.app()), p0, "nothing jogged");
    assert_eq!(message(&g.app()), "");
}
