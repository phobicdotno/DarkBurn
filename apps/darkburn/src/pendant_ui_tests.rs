//! The pendant in the app, against the emulated receiver and the simulator. No real USB device
//! and no real laser.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use darkburn_pendant::{EmulatorHandle, Key, Leds};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::pendant_ui::{self, PendantStatus};
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

const T: Duration = Duration::from_secs(20);

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn state(app: &DarkBurn) -> LaserState {
    app.machine.handle.snapshot().state
}

/// An app whose pendant reaches `emu`, counting how often a connector is made.
fn app_with(emu: &EmulatorHandle) -> (DarkBurn, Arc<AtomicUsize>) {
    let mut a = DarkBurn::new(Prefs::default());
    let made = Arc::new(AtomicUsize::new(0));
    let (e, m) = (emu.clone(), made.clone());
    a.pendant.make_connector = Some(Box::new(move || {
        m.fetch_add(1, Ordering::SeqCst);
        Box::new(e.connector())
    }));
    (a, made)
}

fn pump(ctx: &egui::Context, app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        crate::machine_ui::frame(ctx, app);
        pendant_ui::frame(ctx, app);
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; pendant {:?}, laser {:?}", app.pendant.status, state(app));
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs frames for a while, for checking that something does NOT happen.
fn settle(ctx: &egui::Context, app: &mut DarkBurn) {
    let end = Instant::now() + Duration::from_millis(300);
    while Instant::now() < end {
        crate::machine_ui::frame(ctx, app);
        pendant_ui::frame(ctx, app);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn connected(a: &DarkBurn) -> bool {
    matches!(a.pendant.status, PendantStatus::Connected(_))
}

/// Press and let go of one key, as the receiver reports it.
fn tap(emu: &EmulatorHandle, k: Key) {
    emu.hold(&[k]);
    emu.release();
}

fn connect_sim(ctx: &egui::Context, app: &mut DarkBurn) {
    let id = app.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(app, "device.select", json!({ "id": id })).unwrap();
    run(app, "machine.connect", json!({})).unwrap();
    pump(ctx, app, "the simulator ready", |a| state(a) == LaserState::Ready);
}

fn pos(a: &DarkBurn) -> (f64, f64) {
    let p = a.machine.handle.snapshot().machine_pos;
    (p[0], p[1])
}

#[test]
fn off_by_default_and_nothing_is_opened() {
    let emu = EmulatorHandle::new();
    let (mut a, made) = app_with(&emu);
    assert!(!a.prefs.opts.pendant, "the pendant is off until the user turns it on");
    let ctx = egui::Context::default();
    settle(&ctx, &mut a);
    assert!(!a.pendant.running());
    assert_eq!(made.load(Ordering::SeqCst), 0);
    assert_eq!(emu.connects(), 0);
    assert_eq!(a.pendant.status, PendantStatus::Off);
}

#[test]
fn safe_mode_never_opens_it() {
    let emu = EmulatorHandle::new();
    let (mut a, made) = app_with(&emu);
    a.prefs.opts.pendant = true;
    a.safe_mode = true;
    settle(&egui::Context::default(), &mut a);
    assert_eq!(made.load(Ordering::SeqCst), 0);
    assert_eq!(emu.connects(), 0);
}

#[test]
fn turning_it_on_and_off_opens_and_closes_the_receiver() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    assert_eq!(pendant_ui::status(), a.pendant.status);
    a.prefs.opts.pendant = false;
    pendant_ui::frame(&ctx, &mut a);
    assert!(!a.pendant.running());
    assert_eq!(a.pendant.status, PendantStatus::Off);
}

#[test]
fn keys_do_nothing_without_a_ready_laser() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    for k in [Key::Start, Key::Frame, Key::GoToOrigin, Key::Up, Key::Down, Key::Left, Key::Right, Key::ZPlusW, Key::ZMinusW, Key::Laser] {
        tap(&emu, k);
    }
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Disconnected);
    assert!(a.machine.message.is_none(), "a held-back key is quiet, like a disabled button: {:?}", a.machine.message.as_ref().map(|m| &m.text));
}

#[test]
fn the_gweike_refuses_the_pendant_as_it_refuses_the_buttons() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    let id = run(&mut a, "device.add", json!({ "type": format!("{:?}", DeviceType::GweikeCardSimulator) })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    assert!(!crate::laser_panel::start_allowed(&a));
    assert!(!crate::laser_panel::jog_free(&a));
    for k in [Key::Start, Key::Frame, Key::GoToOrigin, Key::Up, Key::Right, Key::ZPlusW, Key::Laser] {
        tap(&emu, k);
    }
    settle(&ctx, &mut a);
    assert!(!a.machine.handle.is_connected());
    assert_eq!(state(&a), LaserState::Disconnected);
}

#[test]
fn arrows_step_the_head_and_faststep_makes_the_step_longer() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "machine.move_to", json!({ "x": 100, "y": 100 })).unwrap();
    pump(&ctx, &mut a, "the move", |a| state(a) == LaserState::Ready && pos(a) == (100.0, 100.0));
    a.machine.jog.distance = 2.0;
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    let (x0, y0) = pos(&a);
    tap(&emu, Key::Right);
    pump(&ctx, &mut a, "one step right", |a| state(a) == LaserState::Ready && (pos(a).0 - x0 - 2.0).abs() < 1e-3);
    // FastStep held, then Up: ten steps' worth in one go.
    emu.hold(&[Key::FastStep]);
    emu.hold(&[Key::FastStep, Key::Up]);
    emu.release();
    pump(&ctx, &mut a, "a long step up", |a| state(a) == LaserState::Ready && (pos(a).1 - y0 - 20.0).abs() < 1e-3);
    assert!((pos(&a).0 - x0 - 2.0).abs() < 1e-3);
}

#[test]
fn the_laser_key_fires_nothing() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    emu.hold(&[Key::Laser]);
    settle(&ctx, &mut a);
    assert!(!a.machine.handle.snapshot().fire_on);
    emu.release();
    // Fn chords and the keys with no action yet do nothing either.
    emu.hold(&[Key::Fn]);
    emu.hold(&[Key::Fn, Key::GoToOrigin]);
    emu.release();
    for k in [Key::Blow, Key::Follow, Key::Shutter, Key::RedLight, Key::ZCalibrate] {
        tap(&emu, k);
    }
    settle(&ctx, &mut a);
    let s = a.machine.handle.snapshot();
    assert!(!s.fire_on && !s.air_on);
    assert_eq!(s.state, LaserState::Ready);
}

#[test]
fn start_and_stop_run_the_job_like_the_buttons() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 200, "height": 200, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    tap(&emu, Key::Start);
    pump(&ctx, &mut a, "the job running", |a| a.machine.handle.snapshot().state.is_working());
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", |a| !a.machine.handle.snapshot().state.is_working());
}

#[test]
fn permission_denied_shows_the_udev_command() {
    let emu = EmulatorHandle::new();
    emu.deny_permission(true);
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the permission error", |a| matches!(a.pendant.status, PendantStatus::Unavailable(_)));
    let m = a.machine.message.clone().expect("a message");
    assert!(m.error);
    assert!(m.text.contains("70-darkburn-pendant.rules") && m.text.contains("udevadm"), "{}", m.text);
    // Fixed (rule installed, receiver replugged): it connects without a restart.
    emu.deny_permission(false);
    pump(&ctx, &mut a, "the receiver", connected);
}

#[test]
fn leds_follow_the_machine() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    assert_eq!(pendant_ui::leds(&a), Leds::NONE);
    let ctx = egui::Context::default();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    // On connect the current (all off) state is sent once, and nothing more while it stays.
    pump(&ctx, &mut a, "the first LED report", |_| !emu.feature_reports().is_empty());
    settle(&ctx, &mut a);
    let sent = emu.feature_reports();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0][4], 0);
}

#[test]
fn a_settings_file_never_turns_the_pendant_on() {
    let dir = std::env::temp_dir().join(format!("darkburn-pendant-prefs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut theirs = Prefs::default();
    theirs.opts.pendant = true;
    let file = dir.join("theirs.dbprefs");
    std::fs::write(&file, theirs.to_json()).unwrap();
    let mut mine = Prefs::default();
    crate::prefs_files::import(&mut mine, &file, None).unwrap();
    assert!(!mine.opts.pendant, "an imported file switched the pendant on");
    mine.opts.pendant = true;
    crate::prefs_files::import(&mut mine, &file, None).unwrap();
    assert!(mine.opts.pendant, "and does not switch it off either");
    std::fs::write(dir.join(crate::extras::DEFAULTS_FILE), theirs.to_json()).unwrap();
    assert!(!crate::extras::shared_defaults(Some(&dir)).unwrap().opts.pendant, "shared defaults switched the pendant on");
    let _ = std::fs::remove_dir_all(&dir);
}
