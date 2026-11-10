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

// ----- HELP-05: camera selection help ----------------------------------------------------------

#[test]
fn camera_selection_help_is_its_own_topic_and_says_how_to_pick_and_what_to_check() {
    let t = crate::help::topic("camera_selection");
    assert_eq!(t.title, "Choosing a camera");
    assert!(t.paragraphs.len() >= 4);
    let all = t.paragraphs.join(" ");
    for needle in ["Camera Control", "Allow live camera", "Find cameras", "Start", "Not in the list", "one program", "Camera capture", "picture file"]
    {
        assert!(all.contains(needle), "the help mentions {needle}");
    }
    assert_ne!(crate::help::topic("camera").title, t.title, "the panel's own help is another topic");
    // The controls the text names are the ones the Camera Control panel has.
    let src = include_str!("vision_ui.rs");
    for control in ["\"Allow live camera\"", "\"Find cameras\"", "\"Start\""] {
        assert!(src.contains(control), "the panel still has {control}");
    }
}

#[test]
fn help_menu_camera_selection_help_opens_that_topic() {
    let mut g = Gui::standard();
    g.open_menu("Help", "Camera Selection Help");
    g.frames(2);
    assert!(g.app().extras.help_open);
    assert_eq!(g.app().extras.help_topic, "camera_selection");
    assert!(g.window_titles().contains(&"Help".to_string()), "{:?}", g.window_titles());
    assert!(g.has_text("Choosing a camera"), "{:?}", g.visible_texts());
    g.screenshot("help-camera-selection");
    assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());
    // No camera was opened or even listed by reading the help.
    assert!(!g.app().vision.live && g.app().vision.cameras.is_empty());
    g.close_dialogs();
    assert!(!g.app().extras.help_open);
}

// ----- PREV-09: start here, run, send or save from any point in the job ------------------------

fn tick_until(app: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = Instant::now() + T;
    loop {
        app.machine.tick();
        if f(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {:?}", app.machine.handle.snapshot().state);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn connect(app: &mut DarkBurn, kind: DeviceType) {
    let id = match app.machine.devices.devices.iter().find(|d| d.kind == kind) {
        Some(d) => d.id,
        None => run(app, "device.add", json!({ "type": format!("{kind:?}") })).unwrap()["id"].as_u64().unwrap(),
    };
    run(app, "device.select", json!({ "id": id })).unwrap();
    run(app, "machine.connect", json!({})).unwrap();
    tick_until(app, "connected and ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
}

fn finished(app: &DarkBurn) -> usize {
    app.machine.job_log.iter().filter(|l| l.starts_with("Job finished")).count()
}

#[test]
fn from_here_buttons_follow_the_machine_family() {
    use crate::job_ui::from_here_actions;
    use crate::machine_api::Family;
    assert_eq!(from_here_actions(Family::Grbl), (true, false), "G-code: run, no send");
    assert_eq!(from_here_actions(Family::Ruida), (true, true), "DSP: run and send");
    for f in [Family::Galvo, Family::DesignOnly, Family::FileOnly] {
        assert_eq!(from_here_actions(f), (false, false), "{f:?}");
    }
}

#[test]
fn a_dsp_machine_runs_and_stores_a_job_from_a_point_part_way() {
    use crate::machine_api::JobData;
    let mut a = DarkBurn::new(Prefs::default());
    connect(&mut a, DeviceType::SimulatorRuida);
    // Two squares: the later half of the job is the second one.
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 60, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "job.start", Value::Null).unwrap();
    tick_until(&mut a, "whole job", |a| finished(a) == 1);
    tick_until(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let job = a.job.last_job.clone().expect("the job that ran");
    let ops = job.toolpath.ops.len();
    assert!(ops > 8, "{ops} operations");
    let mid = ops / 2;

    // What would be sent: the part-way file is shorter than the whole one, for run and for send.
    let size = |a: &DarkBurn, from: Option<usize>, send: bool| match crate::machine_job::request_for(a, &job, from, send).unwrap().data {
        JobData::RdFile { data, .. } => data.len(),
        JobData::Rd(rd) => rd.to_rd_file(darkburn_ruida::Magic::DEFAULT).unwrap().len(),
        _ => panic!("a DSP job"),
    };
    assert!(size(&a, Some(mid), true) < size(&a, None, true), "send from here leaves the first part out");
    assert!(size(&a, Some(mid), false) < size(&a, None, false), "run from here leaves the first part out");

    // Run from here, then Send from here: the controller runs one and stores the other.
    run(&mut a, "job.preview_run_from_here", json!({ "op_index": mid })).unwrap();
    tick_until(&mut a, "part job", |a| finished(a) == 2);
    tick_until(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    let stored = |a: &DarkBurn| a.machine.handle.snapshot().ruida.files.as_ref().map_or(0, Vec::len);
    let before = stored(&a);
    run(&mut a, "job.preview_send_from_here", json!({ "op_index": mid })).unwrap();
    tick_until(&mut a, "stored", |a| stored(a) > before);
    assert_eq!(finished(&a), 2, "sending does not run it");
    assert!(run(&mut a, "job.preview_run_from_here", json!({})).is_err(), "the point is required");
}

#[test]
fn a_galvo_job_cannot_start_part_way_and_says_so() {
    let mut a = DarkBurn::new(Prefs::default());
    connect(&mut a, DeviceType::SimulatorGalvo);
    run(&mut a, "shape.add_rect", json!({ "x": 40, "y": 40, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "layer.set_param", json!({ "layer": 0, "field": "speed", "value": 800 })).unwrap();
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    let e = run(&mut a, "job.preview_run_from_here", json!({ "op_index": 3 })).unwrap_err();
    assert!(e.contains("part way"), "{e}");
    let e = run(&mut a, "job.preview_send_from_here", json!({ "op_index": 3 })).unwrap_err();
    assert!(e.contains("Use Start"), "{e}");
    for _ in 0..20 {
        a.machine.tick();
    }
    assert_eq!(finished(&a), 0);
    assert!(!a.machine.job_log.iter().any(|l| l.starts_with("Job started")), "nothing was marked: {:?}", a.machine.job_log);
}

// ----- PROJ-05: variable text state, virtual arrays and backup outlines in the project file ----

#[test]
fn a_saved_project_brings_back_variable_text_state_a_virtual_array_and_a_missing_font_outline() {
    use darkburn_core::{ShapeId, ShapeKind};
    let dir = temp("proj05");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("names.csv");
    std::fs::write(&csv, "name,size\nAda,\"10, large\"\nGrace,12\nEdsger,14\n").unwrap();
    let file = dir.join("all three.dbrn");

    let mut a = DarkBurn::new(Prefs::default());
    // Variable text: serial numbers and a CSV file, both moved off their first value.
    run(&mut a, "variable.set", json!({ "start": 5, "end": 50, "current": 9, "advance_by": 2, "padding": 4, "auto_advance": true })).unwrap();
    run(&mut a, "variable.csv_load", json!({ "path": csv.to_string_lossy(), "header": true })).unwrap();
    run(&mut a, "variable.next", Value::Null).unwrap();
    let state = run(&mut a, "variable.get", Value::Null).unwrap();
    assert_eq!(
        (state["current"].as_i64(), state["csv"]["rows"].as_u64(), state["csv"]["current_row"].as_u64()),
        (Some(11), Some(3), Some(1)),
        "{state}"
    );
    // A text in a font this computer does not have: its outline is kept as a backup.
    let text = ShapeId(
        run(&mut a, "text.add", json!({ "text": "Keep me", "x": 10, "y": 60, "height": 10, "font": "A Font Nobody Has", "layer": 0 })).unwrap()["id"]
            .as_u64()
            .unwrap(),
    );
    let outline = a.editor.doc.get(text).unwrap().outline().elements().len();
    assert!(outline > 10);
    // A virtual array: one object, still an array.
    let rect = run(&mut a, "shape.add_rect", json!({ "x": 0, "y": 0, "width": 10, "height": 10, "layer": 1 })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "selection.set", json!({ "ids": [rect] })).unwrap();
    let made = run(&mut a, "arrange.grid_array", json!({ "rows": 2, "cols": 3, "dx": 5, "dy": 5, "virtual": true })).unwrap();
    let array = ShapeId(made["ids"][0].as_u64().unwrap());
    run(&mut a, "project.save_as", json!({ "path": file.to_string_lossy() })).unwrap();

    // Another session opens the file.
    let mut b = DarkBurn::new(Prefs::default());
    run(&mut b, "project.open", json!({ "path": file.to_string_lossy() })).unwrap();
    assert_eq!(run(&mut b, "variable.get", Value::Null).unwrap(), state, "serial numbers, the CSV file and the row it was on");
    assert_eq!(b.editor.doc.variables, a.editor.doc.variables);
    // The variable text carries on from where it was.
    run(&mut b, "variable.next", Value::Null).unwrap();
    let next = run(&mut b, "variable.get", Value::Null).unwrap();
    assert_eq!((next["current"].as_i64(), next["csv"]["current_row"].as_u64()), (Some(13), Some(2)));
    // The text still has its shape although the font is missing.
    let t = b.editor.doc.get(text).expect("the text is there");
    assert!(matches!(&t.kind, ShapeKind::Text(t) if t.backup.is_some() && t.backup_glyphs.is_some()));
    assert_eq!(t.outline().elements().len(), outline);
    // The array is still an array: its settings can be edited.
    let g = b.editor.doc.get(array).expect("the array is there");
    assert!(g.array.is_some() && matches!(&g.kind, ShapeKind::Group { children } if children.len() == 6));
    run(&mut b, "selection.set", json!({ "ids": [array.0] })).unwrap();
    run(&mut b, "arrange.array_edit", json!({ "cols": 4 })).unwrap();
    assert!(matches!(&b.editor.doc.get(array).unwrap().kind, ShapeKind::Group { children } if children.len() == 8));
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- LT-19: material test, galvo variant ------------------------------------------------------

/// A galvo simulator device, selected (not connected), with `source` as its laser source.
fn galvo_device(app: &mut DarkBurn, source: crate::galvo_settings::Source) -> u64 {
    let id = run(app, "device.add", json!({ "type": "SimulatorGalvo" })).unwrap()["id"].as_u64().unwrap();
    app.machine.devices.get_mut(id).unwrap().settings.galvo.source = source;
    run(app, "device.select", json!({ "id": id })).unwrap();
    id
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn galvo_material_test_starts_small_filled_and_inside_the_lens_field() {
    use crate::material_test::{Axis, MaterialTestParams, generate, validate};
    let p = MaterialTestParams::galvo_defaults((110.0, 110.0));
    validate(&p).unwrap();
    assert!(p.galvo && p.fill);
    assert_eq!((p.x_axis, p.y_axis), (Axis::Speed, Axis::Frequency));
    assert!(p.base_interval <= 0.05 && p.base_speed >= 500.0);
    let mut doc = darkburn_core::Document::new();
    let r = generate(&mut doc, &p).unwrap();
    assert!(r.bounds.x0 >= 0.0 && r.bounds.y0 >= 0.0 && r.bounds.x1 <= 110.0 && r.bounds.y1 <= 110.0, "{:?}", r.bounds);
    // A small field still gets a sheet that starts inside it.
    let small = MaterialTestParams::galvo_defaults((20.0, 20.0));
    assert!(small.x >= 0.0 && small.y >= 0.0);
    // The plain test is unchanged and leaves the galvo settings of its layers alone.
    let plain = MaterialTestParams::default();
    assert!(!plain.galvo);
    let mut doc2 = darkburn_core::Document::new();
    let before = doc2.layer(0).main().galvo.frequency_khz;
    generate(&mut doc2, &plain).unwrap();
    assert_eq!(doc2.layer(0).main().galvo.frequency_khz, before);
}

#[test]
fn galvo_material_test_becomes_a_galvo_job_with_its_own_speed_and_frequency_per_cell() {
    use darkburn_galvo::LayerMode;
    let mut a = DarkBurn::new(Prefs::default());
    galvo_device(&mut a, crate::galvo_settings::Source::Fiber);
    let r = run(&mut a, "job.material_test", json!({ "galvo": true, "columns": 3, "rows": 2, "base_power": 35 })).unwrap();
    assert_eq!(r["galvo"], true);
    assert!(r["warnings"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["layers"].as_array().unwrap().len(), 7, "six cells and the labels");

    let dev = a.machine.active().clone();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    let cells: Vec<_> = b.job.layers.iter().filter(|l| matches!(l.mode, LayerMode::Fill(_))).collect();
    let labels: Vec<_> = b.job.layers.iter().filter(|l| matches!(l.mode, LayerMode::Line)).collect();
    assert_eq!((cells.len(), labels.len()), (6, 1));
    // Rows vary the frequency (20 and 60 kHz), columns the speed (500, 1500, 2500 mm/s).
    let mut seen: Vec<(i64, i64)> = cells.iter().map(|l| (l.params.frequency_khz.round() as i64, l.params.speed_mm_s.round() as i64)).collect();
    seen.sort();
    assert_eq!(seen, [(20, 500), (20, 1500), (20, 2500), (60, 500), (60, 1500), (60, 2500)]);
    for l in &cells {
        assert!(near(l.params.power_pct, 35.0));
        assert!(l.params.q_pulse_ns.is_none(), "a Q-switched source has no Q-pulse");
        match &l.mode {
            LayerMode::Fill(f) => assert!(near(f.interval_mm, 0.03)),
            _ => unreachable!(),
        }
    }
    // It compiles for the board, inside the field (centred coordinates).
    let c = crate::galvo_job::compile(&b.job, &dev).unwrap();
    let half = dev.bed().0 / 2.0;
    assert!(c.bounds_mm.x0 >= -half && c.bounds_mm.x1 <= half && c.bounds_mm.y0 >= -half && c.bounds_mm.y1 <= half, "{:?}", c.bounds_mm);
    assert!(!c.list_commands().is_empty());
    // One undo takes the whole sheet away again.
    run(&mut a, "edit.undo", Value::Null).unwrap();
    assert_eq!(a.editor.doc.len(), 0);
}

#[test]
fn galvo_material_test_varies_q_pulse_only_on_a_mopa_source() {
    use darkburn_galvo::LayerMode;
    let args =
        json!({ "galvo": true, "columns": 3, "rows": 1, "x_axis": "q_pulse", "x_min": 50, "x_max": 250, "y_axis": "speed", "base_frequency": 45 });
    let mut a = DarkBurn::new(Prefs::default());
    galvo_device(&mut a, crate::galvo_settings::Source::Fiber);
    let e = run(&mut a, "job.material_test", args.clone()).unwrap_err();
    assert!(e.contains("Q-pulse") && e.contains("MOPA"), "{e}");
    assert_eq!(a.editor.doc.len(), 0, "nothing was added");

    let mut m = DarkBurn::new(Prefs::default());
    galvo_device(&mut m, crate::galvo_settings::Source::Mopa);
    run(&mut m, "job.material_test", args).unwrap();
    let dev = m.machine.active().clone();
    let b = crate::galvo_job::build_job(&m.editor.doc, &dev).unwrap();
    let mut widths: Vec<u32> = b.job.layers.iter().filter(|l| matches!(l.mode, LayerMode::Fill(_))).map(|l| l.params.q_pulse_ns.unwrap()).collect();
    widths.sort();
    assert_eq!(widths, [50, 150, 250]);
    assert!(
        b.job.layers.iter().filter(|l| matches!(l.mode, LayerMode::Fill(_))).all(|l| near(l.params.frequency_khz, 45.0)),
        "the base frequency is on every cell"
    );
    crate::galvo_job::compile(&b.job, &dev).unwrap();
}

#[test]
fn material_test_warns_when_it_cannot_do_what_is_asked_on_this_device() {
    // Frequency on a G-code laser: made, with a warning.
    let mut a = DarkBurn::new(Prefs::default());
    let r = run(&mut a, "job.material_test", json!({ "y_axis": "frequency", "y_min": 20, "y_max": 60 })).unwrap();
    assert!(r["warnings"][0].as_str().unwrap().contains("galvo"), "{r}");
    // A sheet placed outside the lens field: made, with a warning.
    let mut g = DarkBurn::new(Prefs::default());
    galvo_device(&mut g, crate::galvo_settings::Source::Fiber);
    let r = run(&mut g, "job.material_test", json!({ "galvo": true, "x": 100, "y": 100 })).unwrap();
    assert!(r["warnings"][0].as_str().unwrap().contains("does not fit"), "{r}");
}

#[test]
fn galvo_material_test_runs_on_the_galvo_emulator() {
    let mut a = DarkBurn::new(Prefs::default());
    connect(&mut a, DeviceType::SimulatorGalvo);
    run(&mut a, "job.material_test", json!({ "galvo": true, "columns": 2, "rows": 2 })).unwrap();
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    // The board wants the job framed first unless the device says otherwise.
    if let Err(e) = run(&mut a, "machine.start", Value::Null) {
        assert!(e.to_lowercase().contains("fram"), "{e}");
        run(&mut a, "machine.galvo_frame", json!({ "repeat": false })).unwrap();
        tick_until(&mut a, "framed", |a| a.machine.handle.snapshot().state == LaserState::Ready);
        run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
        run(&mut a, "machine.start", Value::Null).unwrap();
    }
    tick_until(&mut a, "marked", |a| finished(a) == 1);
    assert!(a.machine.handle.snapshot().simulator, "only ever the emulator");
}

#[test]
fn material_test_galvo_command_opens_the_window_on_the_galvo_values() {
    let info = crate::commands::describe("laser.material_test_galvo").unwrap();
    assert!(info.implemented && !info.hardware);
    assert_eq!(info.checklist, ["LT-19"]);
    let mut g = Gui::standard();
    g.run_command("laser.material_test_galvo");
    g.frames(2);
    assert!(g.app().job.material_open && g.app().job.material.galvo);
    assert!(g.window_titles().contains(&"Material Test".to_string()), "{:?}", g.window_titles());
    for label in ["Galvo test (frequency and Q-pulse)", "Base frequency", "Base Q-pulse"] {
        assert!(g.has_text(label), "{label}; texts: {:?}", g.visible_texts());
    }
    g.screenshot("material-test-galvo");
    // Unticking the box goes back to the plain test.
    g.click_label("Galvo test (frequency and Q-pulse)");
    assert!(!g.app().job.material.galvo);
    assert!(!g.has_text("Base frequency"));
    g.click_label("Galvo test (frequency and Q-pulse)");
    assert!(g.app().job.material.galvo);
    // Generate adds the sheet to the project.
    g.click_label("Generate");
    g.frames(2);
    assert!(g.app().editor.doc.len() > 25, "25 cells and their labels");
    g.close_dialogs();
}

// ----- GALVO-19: drill mode ---------------------------------------------------------------------

/// A galvo simulator device with a 20 mm square on layer 0, set to drill.
fn drill_app(dwell_ms: f64, spacing_mm: f64) -> DarkBurn {
    let mut a = DarkBurn::new(Prefs::default());
    galvo_device(&mut a, crate::galvo_settings::Source::Fiber);
    run(&mut a, "shape.add_rect", json!({ "x": 40, "y": 40, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "layer.set_mode", json!({ "layer": 0, "mode": "line" })).unwrap();
    let g = &mut a.editor.doc.layer_mut(0).main_mut().galvo;
    g.drill = true;
    g.drill_dwell_ms = dwell_ms;
    g.drill_spacing_mm = spacing_mm;
    a
}

#[test]
fn a_line_layer_set_to_drill_becomes_a_drill_layer_and_other_modes_do_not() {
    use darkburn_galvo::LayerMode;
    let mut a = drill_app(2.0, 5.0);
    let dev = a.machine.active().clone();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert_eq!(b.job.layers.len(), 1);
    assert!(matches!(b.job.layers[0].mode, LayerMode::Drill(d) if near(d.dwell_ms, 2.0) && near(d.spacing_mm, 5.0)), "{:?}", b.job.layers[0].mode);
    let c = crate::galvo_job::compile(&b.job, &dev).unwrap();
    let cmds = c.list_commands();
    let shots = cmds.iter().filter(|c| matches!(c, darkburn_galvo::ListCmd::LaserOnPoint { dwell: 200 })).count();
    assert_eq!(shots, 16, "80 mm round every 5 mm, 2 ms each");
    assert!(!cmds.iter().any(|c| matches!(c, darkburn_galvo::ListCmd::MarkTo { .. })), "no line is traced");
    // The switch is only for line layers: a fill stays a fill.
    run(&mut a, "layer.set_mode", json!({ "layer": 0, "mode": "fill" })).unwrap();
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert!(matches!(b.job.layers[0].mode, LayerMode::Fill(_)));
    // Off again: a traced line.
    run(&mut a, "layer.set_mode", json!({ "layer": 0, "mode": "line" })).unwrap();
    a.editor.doc.layer_mut(0).main_mut().galvo.drill = false;
    let b = crate::galvo_job::build_job(&a.editor.doc, &dev).unwrap();
    assert!(matches!(b.job.layers[0].mode, LayerMode::Line));
}

#[test]
fn drill_settings_are_saved_in_the_project() {
    let a = drill_app(7.5, 1.25);
    let bytes = darkburn_core::project::save(&a.editor.doc, "test", None).unwrap();
    let back = darkburn_core::project::load(&bytes).unwrap().doc;
    let g = back.layer(0).main().galvo;
    assert!(g.drill && near(g.drill_dwell_ms, 7.5) && near(g.drill_spacing_mm, 1.25));
    // A project from before drill mode opens with it off.
    let old: darkburn_core::layer::Galvo = serde_json::from_str(r#"{"frequency_khz":40.0}"#).unwrap();
    assert!(!old.drill && near(old.drill_dwell_ms, 1.0));
}

#[test]
fn a_drill_job_runs_on_the_galvo_emulator_as_shots_in_place() {
    use std::sync::Arc;
    let mut a = drill_app(1.0, 0.0);
    let mut settings = a.machine.active().settings.galvo.clone();
    settings.require_framing = false;
    a.machine.devices.get_mut(a.machine.devices.active.unwrap()).unwrap().settings.galvo = settings.clone();
    let (adapter, emu) = crate::galvo_machine::simulator_with_handle(Arc::new(settings));
    a.machine.handle.attach_simulator(Box::new(adapter), crate::machine_api::Link::SimulatorGalvo);
    tick_until(&mut a, "galvo ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    // Not armed: nothing fires.
    assert!(run(&mut a, "machine.start", Value::Null).is_err());
    assert!(emu.lock().unwrap().log().shots.is_empty());
    run(&mut a, "machine.galvo_arm", json!({ "on": true })).unwrap();
    run(&mut a, "machine.start", Value::Null).unwrap();
    tick_until(&mut a, "drilled", |a| finished(a) == 1);
    let e = emu.lock().unwrap();
    let log = e.log();
    assert_eq!(log.shots.len(), 4, "the four corners: {:?}", log.shots);
    assert!(log.shots.iter().all(|s| s.emitting && s.dwell == 100), "1 ms each with the laser on");
    assert!(!log.motions.iter().any(|m| m.mark && m.emitting), "nothing traced");
    assert!(!e.laser_enabled(), "off afterwards");
}

#[test]
fn cut_settings_window_has_the_drill_controls_on_a_galvo_device_only() {
    let label = "Drill mode (fire at points along the outline)";
    let mut g = Gui::standard();
    g.app().cut_settings_open = Some(0);
    g.frames(2);
    assert!(!g.has_text(label), "a G-code device has no galvo settings");
    g.close_dialogs();
    galvo_device(&mut g.app(), crate::galvo_settings::Source::Fiber);
    g.exec("layer.set_mode", json!({ "layer": 0, "mode": "line" }));
    g.frames(2);
    g.app().cut_settings_open = Some(0);
    g.frames(2);
    g.click_label("Advanced");
    // The galvo settings come after the other advanced ones: scroll down to them.
    let at = g.find_text("Advanced").expect("the tab").center() + egui::vec2(0.0, 200.0);
    for _ in 0..12 {
        if g.has_text(label) {
            break;
        }
        g.scroll(at, egui::vec2(0.0, -300.0));
    }
    g.screenshot("cut-settings-drill");
    assert!(g.has_text(label), "texts: {:?}", g.visible_texts());
    assert!(!g.has_text("Drill time"));
    g.click_label(label);
    assert!(g.has_text("Drill time"), "texts: {:?}", g.visible_texts());
    g.scroll(at, egui::vec2(0.0, -120.0));
    assert!(g.has_text("Point spacing"), "texts: {:?}", g.visible_texts());
    g.screenshot("cut-settings-drill-on");
    assert!(g.app().editor.doc.layer(0).main().galvo.drill, "the layer is set to drill");
    g.close_dialogs();
}
