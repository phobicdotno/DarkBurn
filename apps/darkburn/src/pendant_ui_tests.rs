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
    // A lock of its own, so tests that run at the same time do not hold each other off.
    a.pendant.lock_path = Some(lock_path());
    (a, made)
}

/// A lock file no other test uses.
fn lock_path() -> std::path::PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("darkburn-pendant-locks-{}", std::process::id()));
    dir.join(format!("{}.lock", N.fetch_add(1, Ordering::SeqCst)))
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

/// The window's part after Start: wait for the planned job and hand it on, as a frame does.
fn finish_planning(a: &mut DarkBurn) {
    let result = loop {
        if let Some((task, _)) = a.job.pending.as_mut()
            && let Some(r) = task.poll()
        {
            break r.clone();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (_, then) = a.job.pending.take().unwrap();
    crate::job_cmds::finish(a, result.unwrap(), then);
}

fn job_started(a: &DarkBurn) -> bool {
    a.machine.job_log.iter().any(|l| l.starts_with("Job started"))
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
    finish_planning(&mut a);
    pump(&ctx, &mut a, "the job running", |a| {
        a.machine.handle.snapshot().state.is_working() || a.machine.job_log.iter().any(|l| l.starts_with("Job"))
    });
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", |a| !a.machine.handle.snapshot().state.is_working());
}

/// Start by mistake, Stop at once: the job is still being planned, and it must not start when
/// the planning ends.
#[test]
fn stop_while_the_job_is_being_planned_cancels_the_start() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 200, "height": 200, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    tap(&emu, Key::Start);
    pump(&ctx, &mut a, "the job being planned", |a| a.job.pending.is_some());
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the planned start dropped", |a| a.job.pending.is_none());
    settle(&ctx, &mut a);
    assert!(!state(&a).is_working(), "{:?}", state(&a));
    assert!(!job_started(&a), "{:?}", a.machine.job_log);
}

/// The same through the commands of the Stop button and of an agent, without a pendant.
#[test]
fn the_stop_commands_cancel_a_start_that_is_being_planned() {
    let mut a = DarkBurn::new(Prefs::default());
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 200, "height": 200, "layer": 0 })).unwrap();
    for stop in ["job.stop", "machine.stop", "machine.emergency_stop"] {
        assert!(crate::machine_ui::run_with(&mut a, "job.start", &json!({})));
        assert!(a.job.pending.is_some(), "{stop}: {:?}", a.machine.message.as_ref().map(|m| &m.text));
        if stop == "job.stop" {
            // The button.
            assert!(crate::machine_ui::run_with(&mut a, stop, &json!({})));
        } else {
            // An agent or a script.
            let _ = run(&mut a, stop, json!({}));
        }
        assert!(a.job.pending.is_none(), "{stop} left the planned start waiting");
    }
    settle(&ctx, &mut a);
    assert!(!job_started(&a), "{:?}", a.machine.job_log);
}

/// A start that waits for the out-of-bounds answer or for the checklist is dropped by Stop too.
#[test]
fn stop_drops_a_start_that_waits_for_an_answer() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    // A shape far outside the work area: the planned job asks before it runs.
    run(&mut a, "shape.add_rect", json!({ "x": 100000, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    tap(&emu, Key::Start);
    pump(&ctx, &mut a, "the job being planned", |a| a.job.pending.is_some());
    finish_planning(&mut a);
    assert!(a.job.bounds_prompt.is_some(), "the job leaves the work area, so the question is up");
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the question dropped", |a| a.job.bounds_prompt.is_none());
    // The checklist of a device that has one.
    a.machine.dlg.checklist = Some((Box::new(crate::machine_api::JobRequest::gcode(vec!["G21".into()])), vec![false]));
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the checklist dropped", |a| a.machine.dlg.checklist.is_none());
    assert!(a.machine.finish_checklist().is_err(), "nothing is left to confirm");
    settle(&ctx, &mut a);
    assert!(!job_started(&a), "{:?}", a.machine.job_log);
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

// In Norwegian the status line, the receiver errors and the permission message are Norwegian;
// the install command stays as it is, and English reads as before.
#[test]
fn pendant_texts_follow_the_interface_language() {
    use darkburn_pendant::PendantError;
    crate::i18n::set_language("nb");
    assert_eq!(PendantStatus::Off.text(), "Av");
    assert_eq!(PendantStatus::Looking.text(), "Ser etter mottakeren");
    assert_eq!(PendantStatus::Connected("XHC WHB04B".into()).text(), "Tilkoblet: XHC WHB04B");
    assert_eq!(PendantStatus::Unavailable(PendantError::NotFound).text(), "Ikke tilgjengelig: ingen mottaker for håndkontrollen er satt inn");
    let denied = PendantStatus::Unavailable(PendantError::PermissionDenied { path: "/dev/hidraw3".into() }).text();
    assert_eq!(denied, "Ikke tilgjengelig: ingen tillatelse til å åpne mottakeren for håndkontrollen (/dev/hidraw3)");
    assert_eq!(PendantStatus::InUse.text(), "Brukes av et annet DarkBurn-vindu");
    assert!(PendantStatus::NoLock("x".into()).text().starts_with("Ikke åpnet:"));
    let m = pendant_ui::permission_message();
    assert!(m.starts_with("Mottakeren for håndkontrollen er satt inn"), "{m}");
    assert!(m.ends_with(&format!("\n{}", darkburn_pendant::linux_install_command())), "{m}");

    crate::i18n::set_language("en");
    for e in [
        PendantError::NotFound,
        PendantError::PermissionDenied { path: "/dev/hidraw3".into() },
        PendantError::Unsupported,
        PendantError::Io("gone".into()),
    ] {
        assert_eq!(PendantStatus::Unavailable(e.clone()).text(), format!("Not available: {e}"));
    }
    assert_eq!(PendantStatus::Connected("XHC WHB04B".into()).text(), "Connected: XHC WHB04B");
    assert!(pendant_ui::permission_message().starts_with("The pendant receiver is plugged in, but this user may not open it."));
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

/// Only the person at the computer switches the pendant, in the Settings window: no command
/// does, and no agent.
#[test]
fn no_command_and_no_agent_switches_the_pendant() {
    let emu = EmulatorHandle::new();
    let (mut a, made) = app_with(&emu);
    let ctx = egui::Context::default();
    // The command a script or a hotkey runs, by the option's name ...
    let e = run(&mut a, "setting.set", json!({ "key": "pendant", "value": true })).unwrap_err();
    assert!(e.contains("only be switched on or off by the user"), "{e}");
    assert!(crate::extras::set_option(&mut a, "pendant", &json!(true)).is_err());
    // ... and the agent's own, under both of its names.
    for key in ["pendant", "opts.pendant"] {
        let e = crate::mcp_inspect::set_setting(&mut a, key, &json!(true)).unwrap_err();
        assert!(e.contains("only be switched on or off by the user"), "{key}: {e}");
    }
    assert!(!a.prefs.opts.pendant);
    settle(&ctx, &mut a);
    assert_eq!((made.load(Ordering::SeqCst), emu.connects()), (0, 0), "the receiver was opened without the user's switch");
    // Other settings still go through, and they leave the switch as the user set it.
    a.prefs.opts.pendant = true;
    run(&mut a, "setting.set", json!({ "key": "sound_done", "value": true })).unwrap();
    crate::mcp_inspect::set_setting(&mut a, "opts.snap_grid", &json!(false)).unwrap();
    assert!(a.prefs.opts.pendant && a.prefs.opts.sound_done && !a.prefs.opts.snap_grid);
    // Nor can an agent take the operator's Stop key away by switching it off.
    assert!(crate::mcp_inspect::set_setting(&mut a, "opts.pendant", &json!(false)).is_err());
    assert!(run(&mut a, "setting.set", json!({ "key": "pendant", "value": false })).is_err());
    assert!(a.prefs.opts.pendant);
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

#[test]
fn only_one_window_has_the_receiver_and_another_takes_over_when_it_is_done() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let (mut b, made_b) = app_with(&emu);
    b.pendant.lock_path = a.pendant.lock_path.clone();
    let ctx = egui::Context::default();
    a.prefs.opts.pendant = true;
    b.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    settle(&ctx, &mut b);
    assert_eq!(b.pendant.status, PendantStatus::InUse, "the second window waits");
    assert!(!b.pendant.running());
    assert_eq!(made_b.load(Ordering::SeqCst), 0, "the second window never reached for the receiver");
    assert_eq!(emu.connects(), 1);

    // The first window's keys are its own: the second window's machine stays where it is.
    for app in [&mut a, &mut b] {
        connect_sim(&ctx, app);
        run(app, "machine.move_to", json!({ "x": 100, "y": 100 })).unwrap();
        pump(&ctx, app, "the move", |a| state(a) == LaserState::Ready && pos(a) == (100.0, 100.0));
    }
    tap(&emu, Key::Right);
    pump(&ctx, &mut a, "the first machine moved", |a| state(a) == LaserState::Ready && pos(a).0 > 100.0);
    settle(&ctx, &mut b);
    assert_eq!(pos(&b), (100.0, 100.0), "one key press moved a second machine");

    // Turned off in the first window: the second one takes the receiver.
    a.prefs.opts.pendant = false;
    pendant_ui::logic(&ctx, &mut a);
    assert_eq!(a.pendant.status, PendantStatus::Off);
    pump(&ctx, &mut b, "the second window taking over", connected);
    a.prefs.opts.pendant = true;
    settle(&ctx, &mut a);
    assert_eq!(a.pendant.status, PendantStatus::InUse);

    // A window that goes away lets go too.
    drop(b);
    pump(&ctx, &mut a, "the first window taking it back", connected);
    let lock = a.pendant.lock_path.clone().unwrap();
    drop(a);
    let _ = std::fs::remove_file(lock);
}

// ----- Pause only pauses; Start (Play) resumes a paused job -----------------------------------

fn starts(a: &DarkBurn) -> usize {
    a.machine.job_log.iter().filter(|l| l.starts_with("Job started")).count()
}

/// The pendant's Pause pressed on a paused job leaves it paused, fresh or not: unlike the
/// window's button it never resumes.
#[test]
fn pause_twice_leaves_the_job_paused() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    tap(&emu, Key::Pause);
    pump(&ctx, &mut a, "the job paused", |a| state(a) == LaserState::Paused);
    settle(&ctx, &mut a);
    for round in 0..3 {
        tap(&emu, Key::Pause);
        settle(&ctx, &mut a);
        assert_eq!(state(&a), LaserState::Paused, "Pause number {} set the job going again", round + 2);
    }
    assert!(a.machine.message.as_ref().is_none_or(|m| !m.error), "{:?}", a.machine.message.as_ref().map(|m| &m.text));
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", stopped);
}

/// Pause presses that pile up in the receiver's queue and reach the app together, fresh or long
/// after they were pressed, never set a paused job going.
#[test]
fn pause_presses_replayed_from_the_queue_never_resume() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    tap(&emu, Key::Pause);
    pump(&ctx, &mut a, "the job paused", |a| state(a) == LaserState::Paused);
    // Four at once, read by the poll thread before the next frame takes them: an odd number
    // of toggles would leave the job running.
    for _ in 0..4 {
        tap(&emu, Key::Pause);
    }
    std::thread::sleep(Duration::from_millis(300));
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused, "a burst of Pause presses set the job going again");
    // Three that the app gets to late.
    a.pendant.max_age = Duration::from_millis(200);
    for _ in 0..3 {
        tap(&emu, Key::Pause);
    }
    std::thread::sleep(Duration::from_millis(1200));
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused, "late Pause presses set the job going again");
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", stopped);
}

/// After the pendant sent a pause, another Pause is not sent until the machine shows the job
/// paused or stopped, for at most PAUSE_SETTLE.
#[test]
fn a_second_pause_waits_until_the_machine_shows_the_first() {
    use pendant_ui::{PAUSE_SETTLE, pause_held_back};
    let t = Instant::now();
    let ms = Duration::from_millis;
    assert!(!pause_held_back(LaserState::Busy, None, t), "the first Pause goes out");
    assert!(pause_held_back(LaserState::Busy, Some(t), t), "a second one at once does not");
    assert!(pause_held_back(LaserState::Busy, Some(t), t + ms(100)));
    assert!(pause_held_back(LaserState::Framing, Some(t), t + ms(100)));
    assert!(!pause_held_back(LaserState::Busy, Some(t), t + PAUSE_SETTLE), "a pause that was lost does not block Pause for long");
    assert!(!pause_held_back(LaserState::Paused, Some(t), t), "the machine shows it: nothing to wait for");
    assert!(!pause_held_back(LaserState::Ready, Some(t), t), "the job ended");
    // A press earlier than the pause (a clock read before it) is held back too, not let through.
    assert!(pause_held_back(LaserState::Busy, Some(t + ms(50)), t));
}

/// Start is the pendant's Play: on a paused job it resumes it through the Resume button's
/// command, and only as Start is carried out, fresh and with the window drawn. No new job is
/// planned or started.
#[test]
fn start_resumes_a_paused_job_like_the_resume_button() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    // Something to burn, so that a Start taken for a new job would plan one.
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    tap(&emu, Key::Pause);
    pump(&ctx, &mut a, "the job paused", |a| state(a) == LaserState::Paused);
    assert!(!crate::laser_panel::start_allowed(&a), "the Start button is off on a paused job");
    let started = starts(&a);

    // A Play the app gets to late is dropped, like every key but Stop and Pause.
    a.pendant.max_age = Duration::from_millis(200);
    tap(&emu, Key::Start);
    std::thread::sleep(Duration::from_millis(1200));
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused, "a late Play resumed the job");
    a.pendant.max_age = Duration::from_secs(60);

    // With the window not drawn only the app's logic runs: Play waits for a drawn frame.
    tap(&emu, Key::Start);
    let end = Instant::now() + T;
    while a.pendant.waiting_len() == 0 {
        pendant_ui::logic(&ctx, &mut a);
        assert!(Instant::now() < end, "Play never reached the app");
        std::thread::sleep(Duration::from_millis(5));
    }
    a.machine.tick();
    assert_eq!(state(&a), LaserState::Paused, "Play resumed the job with the window hidden");

    // The window is drawn again: the job goes on.
    pendant_ui::frame(&mut a);
    pump(&ctx, &mut a, "the job going on", |a| state(a) == LaserState::Busy);
    assert!(a.job.pending.is_none(), "Play planned a new job instead of resuming");
    settle(&ctx, &mut a);
    assert_eq!(starts(&a), started, "Play started a new job: {:?}", a.machine.job_log);

    // Pause works again at once, and Play once more.
    tap(&emu, Key::Pause);
    pump(&ctx, &mut a, "the job paused again", |a| state(a) == LaserState::Paused);
    tap(&emu, Key::Start);
    pump(&ctx, &mut a, "the job going on again", |a| state(a) == LaserState::Busy);
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", stopped);
    assert!(a.job.pending.is_none());
}

/// With nothing paused, Play is the Start button as before: nothing while a job runs.
#[test]
fn start_on_a_running_job_does_nothing() {
    let emu = EmulatorHandle::new();
    let (mut a, _) = app_with(&emu);
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    a.prefs.opts.pendant = true;
    pump(&ctx, &mut a, "the receiver", connected);
    start_slow_job(&ctx, &mut a);
    let started = starts(&a);
    tap(&emu, Key::Start);
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Busy);
    assert!(a.job.pending.is_none(), "Start on a running job planned another one");
    assert_eq!(starts(&a), started);
    tap(&emu, Key::Stop);
    pump(&ctx, &mut a, "the job stopped", stopped);
}

/// The window's Pause button is unchanged: on a paused job it resumes. `only_pause`, what the
/// pendant sends, leaves a paused job paused.
#[test]
fn the_pause_button_still_resumes_and_only_pause_does_not() {
    use crate::machine_ui::run_with;
    let mut a = DarkBurn::new(Prefs::default());
    let ctx = egui::Context::default();
    connect_sim(&ctx, &mut a);
    start_slow_job(&ctx, &mut a);
    assert!(run_with(&mut a, "job.pause", &json!({ "only_pause": true })));
    pump(&ctx, &mut a, "only_pause pausing a running job", |a| state(a) == LaserState::Paused);
    assert!(run_with(&mut a, "job.pause", &json!({ "only_pause": true })));
    settle(&ctx, &mut a);
    assert_eq!(state(&a), LaserState::Paused, "only_pause resumed the job");
    assert!(run_with(&mut a, "job.pause", &json!({})));
    pump(&ctx, &mut a, "the button resuming the job", |a| state(a) == LaserState::Busy);
    assert!(run_with(&mut a, "job.stop", &json!({})));
    pump(&ctx, &mut a, "the job stopped", stopped);
}

/// The Settings page says what Pause and Start do on the pendant.
#[test]
fn the_settings_page_says_pause_only_pauses() {
    let mut g = crate::ui_harness::Gui::new(1600.0, 1200.0, 1.0);
    g.app().settings_open = true;
    g.frames(3);
    let win = g.rect_of("dialog:window").expect("the Settings window");
    let tab = g.find_texts("Devices").into_iter().find(|r| win.contains(r.center())).expect("the Devices page in the Settings window");
    g.click_pos(tab.center());
    g.frames(2);
    let text = "Pause only pauses; Start goes on with a paused job.";
    if !g.has_text(text) {
        let win = g.rect_of("dialog:window").unwrap();
        g.scroll(win.center(), egui::vec2(0.0, -2000.0));
        g.frames(2);
    }
    assert!(g.has_text(text), "{:?}", g.visible_texts());
    g.screenshot("pendant-settings");
    g.close_dialogs();
}
