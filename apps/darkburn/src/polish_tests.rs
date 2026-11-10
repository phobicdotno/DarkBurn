//! End-to-end tests of the small finishing features: a real simulator job through the app's
//! frame loop, and the Settings window driven in the headless UI harness.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::devices::DeviceType;
use crate::machine_api::LaserState;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use crate::sounds::Sound;
use crate::ui_harness::Gui;

const T: Duration = Duration::from_secs(20);

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

/// Run the app's machine frame (poll the machine, job log, sounds) until `f` holds.
fn frames_until(app: &mut DarkBurn, ctx: &egui::Context, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        crate::machine_ui::frame(ctx, app);
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("darkburn-polish-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// LASER-25, LASER-26, SET-22, SET-23: a job on the GRBL simulator, from Start to the last line.
#[test]
fn a_simulator_job_plays_the_finished_sound_once_and_adds_a_row_to_the_log() {
    let dir = temp("job");
    let mut a = DarkBurn::new(Prefs::default());
    a.settings_dir = Some(dir.clone());
    a.safe_mode = true;
    let ctx = egui::Context::default();
    run(&mut a, "setting.set", json!({ "key": "sound_done", "value": true })).unwrap();
    run(&mut a, "setting.set", json!({ "key": "sound_error", "value": true })).unwrap();
    run(&mut a, "setting.set", json!({ "key": "job_log", "value": true })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    frames_until(&mut a, &ctx, "connected", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    crate::sounds::take_requests();

    run(&mut a, "machine.start", Value::Null).unwrap();
    frames_until(&mut a, &ctx, "job finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    frames_until(&mut a, &ctx, "ready again", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    assert_eq!(crate::sounds::take_requests(), [Sound::JobDone], "one chime, when the job ended");

    let info = run(&mut a, "job.log_info", Value::Null).unwrap();
    assert_eq!(info["jobs"], 1, "{info}");
    let row = &info["rows"][0];
    assert_eq!(row[2], "Untitled");
    assert_eq!(row[3], a.machine.active().name.as_str());
    assert_eq!(row[4], "finished");
    assert!(row[6].as_str().unwrap().parse::<usize>().unwrap() > 0, "lines sent: {row}");

    // A second job, stopped by hand: a row, and no error sound.
    run(&mut a, "shape.add_rect", json!({ "x": 0, "y": 0, "width": 300, "height": 300, "layer": 0 })).unwrap();
    run(&mut a, "machine.start", Value::Null).unwrap();
    frames_until(&mut a, &ctx, "second job under way", |a| a.job_run.is_running());
    run(&mut a, "machine.stop", Value::Null).unwrap();
    frames_until(&mut a, &ctx, "second job ended", |a| !a.job_run.is_running());
    let info = run(&mut a, "job.log_info", Value::Null).unwrap();
    assert_eq!(info["jobs"], 2, "{info}");
    let second = info["rows"][1][4].as_str().unwrap().to_string();
    if second == "stopped" {
        assert!(crate::sounds::take_requests().is_empty(), "stopping by hand is not an error");
    } else {
        // The simulator was quicker than the stop.
        assert_eq!(second, "finished");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_is_played_or_logged_with_the_settings_off() {
    let dir = temp("off");
    let mut a = DarkBurn::new(Prefs::default());
    a.settings_dir = Some(dir.clone());
    a.safe_mode = true;
    let ctx = egui::Context::default();
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    let id = a.machine.devices.devices.iter().find(|d| d.kind == DeviceType::SimulatorGrbl).unwrap().id;
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "machine.connect", json!({})).unwrap();
    frames_until(&mut a, &ctx, "connected", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    crate::sounds::take_requests();
    run(&mut a, "machine.start", Value::Null).unwrap();
    frames_until(&mut a, &ctx, "job finish", |a| a.machine.job_log.iter().any(|l| l.starts_with("Job finished")));
    assert!(crate::sounds::take_requests().is_empty());
    assert!(!dir.join(crate::settings_wiring::JOB_LOG_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// The Settings window: the switches, the try buttons, the folder picker and Show the job log.
#[test]
fn settings_window_has_the_sound_and_job_log_controls_and_they_work() {
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.frames(3);
    for label in ["Play a sound on errors", "Play a sound when a job finishes", "Save a log of each job", "Job log folder"] {
        assert!(g.has_text(label), "{label} is in Settings; texts: {:?}", g.visible_texts());
    }
    g.screenshot("settings-sounds-job-log");
    assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());
    assert!(!g.app().prefs.opts.sound_done && !g.app().prefs.opts.job_log);
    g.click_label("Play a sound when a job finishes");
    g.click_label("Play a sound on errors");
    g.click_label("Save a log of each job");
    {
        let a = g.app();
        assert!(a.prefs.opts.sound_done && a.prefs.opts.sound_error && a.prefs.opts.job_log);
    }
    crate::sounds::take_requests();
    g.click_label("Try the finished sound");
    assert_eq!(crate::sounds::take_requests(), [Sound::JobDone]);
    g.click_label("Try the error sound");
    assert_eq!(crate::sounds::take_requests(), [Sound::Error]);

    let dir = g.temp("job-logs");
    g.answer_dialog(&dir);
    g.click_label("Choose the log folder");
    assert_eq!(g.app().prefs.opts.job_log_dir, dir.display().to_string());
    assert!(g.dialog_log().iter().any(|l| l.contains("pick_folder")), "{:?}", g.dialog_log());
    crate::misc_cmds::take_revealed();
    g.click_label("Show the job log");
    assert_eq!(crate::misc_cmds::take_revealed(), std::slice::from_ref(&dir));
    g.close_dialogs();
}
