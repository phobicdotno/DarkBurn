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

// ----- SET-05: dark background, grid contrast, grid spacing ------------------------------------

/// The pixels of the row `y_mm` below the page's top edge, between its left and right edges.
fn page_row(g: &mut Gui, y_mm: f32) -> Vec<[u8; 3]> {
    g.frames(2);
    let (w, _) = {
        let a = g.app();
        (a.prefs.page_width, a.prefs.page_height)
    };
    let scale = g.scale();
    let from = g.mm_to_screen(0.0, y_mm);
    let to = g.mm_to_screen(w, y_mm);
    let img = g.render_image();
    let y = ((from.y * scale) as u32).min(img.height() - 1);
    let (x0, x1) =
        (((from.x * scale).ceil() as u32 + 2).min(img.width() - 1), ((to.x * scale).floor() as u32).saturating_sub(2).min(img.width() - 1));
    (x0..x1)
        .map(|x| {
            let p = img.get_pixel(x, y).0;
            [p[0], p[1], p[2]]
        })
        .collect()
}

/// The most common colour of a row (the page between the grid lines).
fn fill_of(row: &[[u8; 3]]) -> [u8; 3] {
    let mut counts = std::collections::HashMap::new();
    for p in row {
        *counts.entry(*p).or_insert(0usize) += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(p, _)| p).unwrap()
}

fn distance(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3).map(|i| (i32::from(a[i]) - i32::from(b[i])).abs()).sum()
}

/// How many grid lines cross the row: runs of pixels that are not the page's own colour.
fn lines_in(row: &[[u8; 3]]) -> usize {
    let fill = fill_of(row);
    let mut n = 0;
    let mut inside = false;
    for p in row {
        let line = distance(*p, fill) > 6;
        if line && !inside {
            n += 1;
        }
        inside = line;
    }
    n
}

fn strongest_line(row: &[[u8; 3]]) -> i32 {
    let fill = fill_of(row);
    row.iter().map(|p| distance(*p, fill)).max().unwrap_or(0)
}

/// A design-only window with only the canvas in the way of the page: no panels on the right.
fn canvas_gui() -> Gui {
    let mut g = Gui::standard();
    g.exec("setting.set", json!({ "key": "grid_contrast", "value": 1.0 }));
    g.frames(2);
    g
}

#[test]
fn grid_spacing_setting_changes_the_lines_on_the_canvas() {
    let mut g = canvas_gui();
    assert_eq!(g.app().prefs.grid_spacing, 10.0, "10 mm by default");
    // 7 mm below the top edge: between two horizontal lines for every spacing used here.
    let at_10 = lines_in(&page_row(&mut g, 7.0));
    g.exec("setting.set", json!({ "key": "grid_spacing", "value": 50.0 }));
    let at_50 = lines_in(&page_row(&mut g, 7.0));
    g.exec("setting.set", json!({ "key": "grid_spacing", "value": 100.0 }));
    let at_100 = lines_in(&page_row(&mut g, 7.0));
    g.screenshot("grid-spacing-100");
    // A 400 mm page: lines inside it at 10, 20 .. 390, at 50 .. 350, at 100 .. 300.
    assert_eq!((at_10, at_50, at_100), (39, 7, 3), "lines across the page");
    // Hidden grid: no lines at all.
    g.exec("setting.set", json!({ "key": "show_grid", "value": false }));
    assert_eq!(lines_in(&page_row(&mut g, 7.0)), 0);
}

#[test]
fn grid_contrast_setting_makes_the_lines_stronger_or_fainter() {
    let mut g = canvas_gui();
    let strong = strongest_line(&page_row(&mut g, 7.0));
    g.exec("setting.set", json!({ "key": "grid_contrast", "value": 0.0 }));
    let faint = strongest_line(&page_row(&mut g, 7.0));
    assert!(faint > 0, "the faintest grid is still there");
    assert!(strong > faint * 2, "full contrast {strong} against the faintest {faint}");
}

#[test]
fn background_setting_makes_the_canvas_dark_or_light_whatever_the_theme() {
    let mut g = canvas_gui();
    let luma = |p: [u8; 3]| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]);
    g.exec("setting.set", json!({ "key": "backdrop", "value": "dark" }));
    let dark = luma(fill_of(&page_row(&mut g, 7.0)));
    g.exec("setting.set", json!({ "key": "backdrop", "value": "light" }));
    let light = luma(fill_of(&page_row(&mut g, 7.0)));
    g.screenshot("backdrop-light");
    assert!(dark < 3 * 90, "a dark page: {dark}");
    assert!(light > 3 * 170, "a light page: {light}");
    // The light theme with the dark background is still dark.
    g.exec("setting.set", json!({ "key": "theme", "value": "light" }));
    g.exec("setting.set", json!({ "key": "backdrop", "value": "dark" }));
    let dark_in_light = luma(fill_of(&page_row(&mut g, 7.0)));
    assert!(dark_in_light < 3 * 90, "{dark_in_light}");
    assert!(execute_headless(&mut g.app(), "setting.set", &json!({ "key": "backdrop", "value": "pink" })).is_err());
}

#[test]
fn display_settings_page_has_the_background_contrast_and_spacing_controls() {
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.frames(2);
    g.click_label("Display");
    for label in ["Background", "Grid contrast", "Grid spacing", "Show the grid"] {
        assert!(g.has_text(label), "{label}; texts: {:?}", g.visible_texts());
    }
    assert!(g.has_text("0 follows the zoom."), "the spacing row explains 0: {:?}", g.visible_texts());
    g.close_dialogs();
}

#[test]
fn each_device_remembers_its_grid_spacing() {
    use crate::devices::Device;
    let mut a = DarkBurn::new(Prefs::default());
    crate::machine_ui::adopt_device_state(&mut a);
    let first = a.machine.devices.active.unwrap();
    run(&mut a, "setting.set", json!({ "key": "grid_spacing", "value": 25.0 })).unwrap();
    let mut d = Device::new(DeviceType::Grbl);
    d.name = "Fine grid".into();
    d.state.grid_spacing = 2.5;
    d.state.panel_layout = serde_json::to_string(&a.prefs.panels).ok();
    let id = a.machine.devices.add(d);
    a.machine.select_device(id);
    crate::machine_ui::adopt_device_state(&mut a);
    assert_eq!(a.prefs.grid_spacing, 2.5, "the new device's spacing is on the canvas");
    assert_eq!(a.machine.devices.get(first).unwrap().state.grid_spacing, 25.0, "the old device kept its own");
    a.prefs.grid_spacing = 0.1;
    a.machine.select_device(first);
    crate::machine_ui::adopt_device_state(&mut a);
    assert_eq!(a.prefs.grid_spacing, 25.0);
    assert_eq!(a.machine.devices.get(id).unwrap().state.grid_spacing, 0.1, "no float noise in the device file");
}

// ----- LIB-04: type icon for line, fill and image entries --------------------------------------

#[test]
fn library_entries_are_sorted_into_line_fill_and_image_kinds() {
    use crate::library::EntryKind;
    use darkburn_core::Mode;
    assert_eq!(EntryKind::of(Mode::Line), EntryKind::Line);
    assert_eq!(EntryKind::of(Mode::Image), EntryKind::Image);
    for m in [Mode::Fill, Mode::FillLine, Mode::OffsetFill] {
        assert_eq!(EntryKind::of(m), EntryKind::Fill);
    }
    let ids: Vec<&str> = Mode::ALL.iter().map(|m| EntryKind::of(*m).id()).collect();
    assert_eq!(ids, ["line", "fill", "fill", "fill", "image"]);
}

#[test]
fn library_panel_draws_a_different_icon_for_line_fill_and_image_entries() {
    let mut g = Gui::standard();
    // Show the panel first: its first drawing loads the device's own library.
    g.frames(2);
    g.click_id("panel:tab:library");
    g.frames(2);
    for (layer, mode, name) in [(0, "line", "Cut through"), (1, "fill", "Engrave dark"), (2, "line", "Photo")] {
        g.exec("layer.set_mode", json!({ "layer": layer, "mode": mode }));
        let made = g.exec("library.create_from_layer", json!({ "layer": layer, "material": "Birch", "thickness": 3.0, "description": name }));
        if name == "Photo" {
            // An image entry (a layer only takes the image mode once it holds a picture).
            let i = made["entry"].as_u64().unwrap() as usize;
            g.app().extras.library.lib.entries[i].settings[0].mode = darkburn_core::Mode::Image;
        }
    }
    g.frames(2);
    for name in ["Cut through", "Engrave dark", "Photo"] {
        assert!(g.has_text(name), "{name} is listed; texts: {:?}", g.visible_texts());
    }
    assert!(!g.visible_texts().iter().any(|t| t.contains("[LINE]") || t.contains("[FILL]") || t.contains("[IMG]")), "no text tags any more");
    let icons = g.rects("library:kind:");
    let kinds: Vec<&str> = icons.iter().map(|(n, _)| n.split(':').nth(2).unwrap()).collect();
    assert_eq!(icons.len(), 3, "{icons:?}");
    for k in ["line", "fill", "image"] {
        assert!(kinds.contains(&k), "{kinds:?}");
    }
    g.screenshot("library-kind-icons");
    // The three icons are three different pictures: count the inked pixels of each.
    let scale = g.scale();
    let img = g.render_image();
    let inked = |r: egui::Rect| -> usize {
        let bg = img.get_pixel((r.left() * scale) as u32, (r.top() * scale) as u32).0;
        let mut n = 0;
        for y in (r.top() * scale) as u32..(r.bottom() * scale) as u32 {
            for x in (r.left() * scale) as u32..(r.right() * scale) as u32 {
                let p = img.get_pixel(x, y).0;
                if (0..3).map(|i| (i32::from(p[i]) - i32::from(bg[i])).abs()).sum::<i32>() > 60 {
                    n += 1;
                }
            }
        }
        n
    };
    let count = |kind: &str| inked(icons.iter().find(|(n, _)| n.split(':').nth(2) == Some(kind)).unwrap().1);
    let (line, fill, image) = (count("line"), count("fill"), count("image"));
    assert!(line >= 20, "the line icon is an outline: {line}");
    assert!(fill >= line + 20, "the fill icon is solid: {fill} against {line}");
    assert!(image != line && image != fill, "the image icon is a third picture: line {line}, fill {fill}, image {image}");
    // Clicking an icon picks its entry.
    let photo = icons.iter().find(|(n, _)| n.starts_with("library:kind:image")).unwrap().1;
    g.click_pos(photo.center());
    let picked = g.app().extras.library.selected;
    assert_eq!(picked.map(|i| g.app().extras.library.lib.entries[i].description.clone()).as_deref(), Some("Photo"));
}
