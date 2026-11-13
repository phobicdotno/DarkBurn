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
    // A loaded test computer can stall for longer than a key press may be old in the app. The
    // tests about late presses set their own limit.
    assert_eq!(a.pendant.max_age, pendant_ui::MAX_AGE);
    a.pendant.max_age = Duration::from_secs(60);
    (a, made)
}

/// One drawn frame, as far as the pendant goes: the app's logic, then the window's part.
fn frame(ctx: &egui::Context, app: &mut DarkBurn) {
    pendant_ui::logic(ctx, app);
    crate::machine_ui::frame(ctx, app);
    pendant_ui::frame(app);
}

fn pump(ctx: &egui::Context, app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        frame(ctx, app);
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
        frame(ctx, app);
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

/// A G-code job that runs for a good minute on the simulator, started as the window starts one.
fn start_slow_job(ctx: &egui::Context, a: &mut DarkBurn) {
    let mut j = crate::machine_api::JobRequest::gcode(
        ["G21 G90".to_owned(), "M4 S100".to_owned()].into_iter().chain((0..60).map(|i| format!("G1 X{} Y10 F300", 10 + (i % 2) * 30))).collect(),
    );
    j.name = "slow".into();
    a.machine.begin_start(j).unwrap();
    pump(ctx, a, "the slow job running", |a| state(a) == LaserState::Busy);
}

fn stopped(a: &DarkBurn) -> bool {
    matches!(state(a), LaserState::Ready | LaserState::Alarm)
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
    pendant_ui::logic(&ctx, &mut a);
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
    // Start takes the button's path: the job is planned off the UI thread first.
    pump(&ctx, &mut a, "the job being planned", |a| a.job.pending.is_some());
    let result = loop {
        if let Some((task, _)) = a.job.pending.as_mut()
            && let Some(r) = task.poll()
        {
            break r.clone();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (_, then) = a.job.pending.take().unwrap();
    crate::job_cmds::finish(&mut a, result.unwrap(), then);
    pump(&ctx, &mut a, "the job running", |a| {
        a.machine.handle.snapshot().state.is_working() || a.machine.job_log.iter().any(|l| l.starts_with("Job"))
    });
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", |a| !a.machine.handle.snapshot().state.is_working());
}

/// A thumb resting on Fn must not take Stop or Pause away.
#[test]
fn stop_and_pause_work_with_fn_held() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    emu.hold(&[Key::Fn]);
    emu.hold(&[Key::Fn, Key::Pause]);
    pump(&ctx, &mut a, "the job paused with Fn held", |a| state(a) == LaserState::Paused);
    emu.hold(&[Key::Fn]);
    emu.hold(&[Key::Fn, Key::Stop]);
    pump(&ctx, &mut a, "the job stopped with Fn held", stopped);
    emu.release();
}

/// A minimised or covered window is not drawn: eframe then runs the app's logic and nothing
/// else. This test runs exactly that, with no UI pass at all, and Stop and Pause must work.
#[test]
fn stop_and_pause_work_while_the_window_is_not_drawn() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    // From here on the window is hidden: only the app's logic runs, the way eframe runs it.
    let app = std::rc::Rc::new(std::cell::RefCell::new(a));
    let mcp = crate::mcp_ui::Host::new(app.clone());
    let mut shell = crate::App { app: app.clone(), writer: crate::PrefsWriter::default(), frames: 2, mcp };
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let mut hidden = |what: &str, done: &dyn Fn(&DarkBurn) -> bool| {
        let end = Instant::now() + T;
        loop {
            let _ = ctx.run_logic(&egui::RawInput::default(), |ctx| eframe::App::logic(&mut shell, ctx, &mut eframe_frame));
            let mut a = app.borrow_mut();
            // The machine's own thread does the work; this only reads where it got to.
            a.machine.tick();
            if done(&a) {
                return;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}; pendant {:?}, laser {:?}", a.pendant.status, state(&a));
            drop(a);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    tap(&emu, Key::Pause);
    hidden("the job paused with the window hidden", &|a: &DarkBurn| state(a) == LaserState::Paused);
    tap(&emu, Key::Stop);
    hidden("the job stopped with the window hidden", &stopped);
    // The keys that move or start something wait for a drawn frame, so they did nothing.
    tap(&emu, Key::Up);
    tap(&emu, Key::Start);
    let before = pos(&app.borrow());
    hidden("the keys taken", &|a: &DarkBurn| a.pendant.waiting_len() == 2);
    assert_eq!(pos(&app.borrow()), before);
    assert!(app.borrow().job.pending.is_none(), "Start was carried out with the window hidden");
}

/// The window thread did not run for a while (a file dialog was open, the window was hidden):
/// the keys pressed meanwhile are not carried out when it runs again.
#[test]
fn keys_pressed_while_the_app_was_busy_are_not_carried_out_late() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "machine.move_to", json!({ "x": 100, "y": 100 })).unwrap();
    pump(&ctx, &mut a, "the move", |a| state(a) == LaserState::Ready && pos(a) == (100.0, 100.0));
    a.machine.jog.distance = 10.0;
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    a.pendant.max_age = Duration::from_millis(200);
    // Five steps with FastStep held (500 mm in all) and Start, with no frame in between.
    emu.hold(&[Key::FastStep]);
    for _ in 0..5 {
        emu.hold(&[Key::FastStep, Key::Up]);
        emu.hold(&[Key::FastStep]);
    }
    emu.release();
    tap(&emu, Key::Start);
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        crate::laser_panel::start_allowed(&a) && crate::laser_panel::jog_free(&a),
        "the buttons are enabled, so only the age holds the keys back"
    );
    settle(&ctx, &mut a);
    assert_eq!(pos(&a), (100.0, 100.0), "a late arrow moved the head");
    assert!(a.job.pending.is_none(), "a late Start began planning a job");
    assert_eq!(state(&a), LaserState::Ready);
    // A fresh press works again at once.
    a.pendant.max_age = Duration::from_secs(60);
    tap(&emu, Key::Up);
    pump(&ctx, &mut a, "a fresh step", |a| state(a) == LaserState::Ready && (pos(a).1 - 110.0).abs() < 1e-3);
}

/// A Pause the app gets to late must not set a paused job going again; a late Stop still stops.
#[test]
fn a_late_pause_never_resumes_and_a_late_stop_still_stops() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    tap(&emu, Key::Pause);
    pump(&ctx, &mut a, "the job paused", |a| state(a) == LaserState::Paused);
    a.pendant.max_age = Duration::from_millis(200);
    tap(&emu, Key::Pause);
    std::thread::sleep(Duration::from_millis(1200));
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused, "a Pause pressed long ago set the job going again");
    tap(&emu, Key::Stop);
    std::thread::sleep(Duration::from_millis(1200));
    pump(&ctx, &mut a, "the late Stop", stopped);
}

/// Two Pause presses that reach the app together are a replay or a radio drop-out: the job is
/// paused, not paused and set going again.
#[test]
fn two_pause_presses_at_once_pause_once() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    tap(&emu, Key::Pause);
    tap(&emu, Key::Pause);
    // Both are read by the poll thread before the next frame takes them.
    std::thread::sleep(Duration::from_millis(300));
    pump(&ctx, &mut a, "the job paused", |a| state(a) == LaserState::Paused);
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused);
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
