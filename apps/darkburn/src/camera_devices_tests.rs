//! Finding, starting and stopping cameras [CAMP-01, CAMP-02, CAM-01, SET-37]. Every test here
//! uses the simulated camera; under `cargo test` the system cameras refuse to open.
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use crate::ui_harness::Gui;

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

/// An app set to the simulated camera at `w` x `h`, with live camera allowed.
fn simulated(w: u32, h: u32) -> DarkBurn {
    let mut a = app();
    a.prefs.opts.capture_system = "simulated".into();
    a.prefs.opts.capture_width = w;
    a.prefs.opts.capture_height = h;
    a.vision.prefs.live_enabled = true;
    a
}

#[test]
fn opening_a_camera_is_a_hardware_command_and_stop_is_always_allowed() {
    for id in ["camera.find", "camera.start", "camera.stop"] {
        assert!(crate::commands::hardware(id), "{id} is hardware");
        assert!(crate::mcp_catalog::is_hardware(id), "{id} goes through the agent gate");
        assert!(!crate::commands::checklist(id).is_empty() || !crate::vision_cmds::checklist(id).is_empty(), "{id} names its checklist items");
    }
    use crate::mcp_catalog::hardware_class;
    use darkburn_mcp::HardwareClass;
    assert_eq!(hardware_class("camera.find"), Some(HardwareClass::Connect));
    assert_eq!(hardware_class("camera.start"), Some(HardwareClass::Connect));
    assert_eq!(hardware_class("camera.stop"), Some(HardwareClass::Stop));
    // The other camera commands only change settings or read files.
    assert!(!crate::commands::hardware("camera.set"));
    assert!(!crate::commands::hardware("camera.update_overlay"));
}

#[test]
fn tests_never_reach_a_real_camera() {
    let mut a = app();
    a.vision.prefs.live_enabled = true;
    for system in ["default", "media_foundation", "video4linux", "avfoundation"] {
        a.prefs.opts.capture_system = system.into();
        let e = run(&mut a, "camera.find", json!({})).unwrap_err();
        assert!(e.contains("without live camera"), "{system}: {e}");
        assert!(run(&mut a, "camera.start", json!({ "index": 0 })).is_err(), "{system}");
        assert!(!a.vision.live && a.vision.source.is_none());
    }
}

#[test]
fn find_lists_the_cameras_of_the_chosen_capture_system() {
    let mut a = simulated(640, 480);
    let r = run(&mut a, "camera.find", json!({})).unwrap();
    assert_eq!(r["system"], "simulated");
    let names: Vec<&str> = r["cameras"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Simulated overhead camera", "Simulated head camera"]);
    assert_eq!(a.vision.cameras.len(), 2);
}

#[test]
fn start_needs_permission_and_a_camera() {
    let mut a = simulated(640, 480);
    a.vision.prefs.live_enabled = false;
    assert!(run(&mut a, "camera.start", json!({ "index": 0 })).unwrap_err().contains("Allow live camera"));
    a.vision.prefs.live_enabled = true;
    assert!(run(&mut a, "camera.start", json!({})).unwrap_err().contains("Pick a camera"));
    assert!(run(&mut a, "camera.start", json!({ "index": 7 })).unwrap_err().contains("no camera 7"));
    assert!(!a.vision.live);
}

#[test]
fn start_opens_at_the_size_from_settings_and_remembers_the_camera() {
    let mut a = simulated(640, 480);
    let r = run(&mut a, "camera.start", json!({ "index": 1 })).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(640), Some(480)));
    assert!(a.vision.live && a.vision.source.is_some());
    assert_eq!(a.vision.raw.as_ref().map(|i| i.dimensions()), Some((640, 480)), "the viewfinder has the first picture at once");
    assert_eq!(a.vision.state.camera.device_index, Some(1));
    assert_eq!(a.vision.state.camera.camera_name, "Simulated head camera");
    assert_eq!(run(&mut a, "camera.get", json!({})).unwrap()["live"], true);
    // Another size in Settings is used the next time a camera opens.
    run(&mut a, "camera.stop", json!({})).unwrap();
    assert!(!a.vision.live && a.vision.source.is_none());
    assert!(a.vision.raw.is_some(), "the last picture stays after Stop");
    a.prefs.opts.capture_width = 320;
    a.prefs.opts.capture_height = 240;
    run(&mut a, "camera.start", json!({})).unwrap();
    assert_eq!(a.vision.raw.as_ref().map(|i| i.dimensions()), Some((320, 240)), "Start without an index opens the chosen camera");
}

/// The exposure and brightness saved with the camera settings reach the camera when it opens,
/// and a change while it runs reaches it at once [CAM-01].
#[test]
fn exposure_and_brightness_reach_the_running_camera() {
    let grey = |a: &mut DarkBurn| {
        let img = a.vision.source.as_mut().unwrap().grab().unwrap();
        img.get_pixel(4, 4)[0]
    };
    let mut a = simulated(320, 240);
    run(&mut a, "camera.start", json!({ "index": 0 })).unwrap();
    let plain = grey(&mut a);
    run(&mut a, "camera.set", json!({ "brightness": 60.0 })).unwrap();
    let brighter = grey(&mut a);
    assert!(brighter > plain, "brightness while running: {plain} -> {brighter}");
    run(&mut a, "camera.set", json!({ "exposure": -8.0 })).unwrap();
    let darker = grey(&mut a);
    assert!(darker < brighter, "a shorter exposure darkens: {brighter} -> {darker}");
    // Saved settings are given to the camera when it is opened again.
    run(&mut a, "camera.stop", json!({})).unwrap();
    let r = run(&mut a, "camera.start", json!({})).unwrap();
    assert!(r["controls"].as_array().unwrap().iter().all(|c| c["ok"] == true), "{r}");
    assert_eq!(r["controls"].as_array().unwrap().len(), 2);
    assert_eq!(grey(&mut a), darker, "the reopened camera has the saved exposure and brightness");
}

#[test]
fn the_black_and_white_view_keeps_brightness_and_drops_colour() {
    let raw = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 255]));
    assert_eq!(*crate::camera_devices::viewfinder(&raw, true), raw);
    let g = crate::camera_devices::viewfinder(&raw, false);
    let p = g.get_pixel(0, 0);
    assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    assert_eq!(p[0], 124);
}

/// Click the camera panel's own `label` button: the one on the row of "Find cameras" (the Laser
/// panel has Start and Stop buttons too).
fn click_in_camera_row(g: &mut Gui, label: &str) {
    let row = g.find_text("Find cameras").expect("the camera buttons are shown");
    let r = g
        .find_texts(label)
        .into_iter()
        .filter(|r| (r.center().y - row.center().y).abs() < 4.0)
        .min_by(|a, b| (a.center().x - row.center().x).abs().total_cmp(&(b.center().x - row.center().x).abs()))
        .unwrap_or_else(|| panic!("no {label} button beside Find cameras"));
    g.click_pos(r.center());
}

fn show_camera_panel(g: &mut Gui) {
    if !g.app().prefs.panel("camera").visible {
        g.run_command("panel.camera");
    }
    let area = g.app().prefs.panel("camera").area;
    g.app().prefs.active_tab.insert(area, "camera".to_string());
    g.frames(3);
}

/// The Camera Control panel with the simulated camera: Find cameras fills the picker, Start
/// opens the chosen one and the viewfinder follows its pictures, Stop closes it [CAMP-01, CAMP-02].
#[test]
fn the_camera_panel_finds_starts_and_stops_a_camera() {
    let mut g = Gui::standard();
    {
        let mut a = g.app();
        a.prefs.opts.capture_system = "simulated".into();
        a.prefs.opts.capture_width = 400;
        a.prefs.opts.capture_height = 300;
        a.vision.prefs.live_enabled = true;
    }
    show_camera_panel(&mut g);
    assert!(g.has_text("Find cameras"), "the picker is offered with the simulated camera");
    g.click_label("Find cameras");
    g.frames(2);
    assert_eq!(g.app().vision.cameras.len(), 2);
    // Start without a choice asks for one.
    click_in_camera_row(&mut g, "Start");
    g.frames(2);
    assert!(!g.app().vision.live);
    assert!(g.has_text("Pick a camera first."));
    g.exec("camera.set", json!({ "device_index": 0, "camera_name": "Simulated overhead camera" }));
    g.frames(2);
    click_in_camera_row(&mut g, "Start");
    g.frames(2);
    assert!(g.app().vision.live, "Start opened the camera");
    let first = g.app().vision.frame_gen;
    g.frames(6);
    assert!(g.app().vision.frame_gen > first + 2, "the viewfinder follows the camera");
    assert!(!g.has_text("No picture yet."));
    g.screenshot("camera_panel_live_simulated");
    // Black and white view.
    g.app().prefs.opts.capture_colour = false;
    g.frames(3);
    g.screenshot("camera_panel_live_simulated_bw");
    click_in_camera_row(&mut g, "Stop");
    g.frames(2);
    assert!(!g.app().vision.live && g.app().vision.source.is_none());
    let stopped = g.app().vision.frame_gen;
    g.frames(4);
    assert_eq!(g.app().vision.frame_gen, stopped, "no pictures after Stop");
}

// ----- Trace from the camera [IMG-20] ---------------------------------------------------------

fn images(a: &DarkBurn) -> usize {
    a.editor.doc.iter().filter(|s| matches!(s.kind, darkburn_core::ShapeKind::Image(_))).count()
}

/// The bytes of the one picture in the document.
fn picture(a: &DarkBurn) -> Vec<u8> {
    let d = &a.editor.doc;
    let key = d.iter().find_map(|s| if let darkburn_core::ShapeKind::Image(i) = &s.kind { Some(i.image.clone()) } else { None }).expect("a picture");
    d.images.get(&key).expect("its image data").bytes.to_vec()
}

fn paths(a: &DarkBurn) -> usize {
    a.editor.doc.iter().filter(|s| !matches!(s.kind, darkburn_core::ShapeKind::Image(_))).count()
}

/// Trace takes a picture, Cancel takes it away again with no undo step left behind, and OK
/// leaves only the traced paths as one undo step.
#[test]
fn trace_from_the_camera_takes_a_picture_that_cancel_takes_away() {
    let mut a = simulated(320, 240);
    run(&mut a, "camera.start", json!({ "index": 0 })).unwrap();
    let steps = a.editor.history.len();
    crate::capture_trace::open(&mut a).unwrap();
    let d = a.images.trace.take().expect("the Trace Image window opens");
    assert!(d.args.delete_image, "the picture is only there for the trace");
    assert_eq!(images(&a), 1);
    crate::capture_trace::cancel(&mut a, &d);
    assert_eq!((a.editor.doc.len(), a.editor.history.len()), (0, steps), "Cancel leaves nothing behind");
    assert!(!a.editor.history.redo_label().is_some_and(|l| l == "Camera picture"), "and nothing to redo either");

    crate::capture_trace::open(&mut a).unwrap();
    let d = a.images.trace.take().unwrap();
    let r = crate::capture_trace::finish(&mut a, &d).unwrap();
    assert!(r.nodes > 0);
    assert_eq!((images(&a), paths(&a)), (0, 1), "only the traced paths stay");
    assert_eq!(a.editor.history.len(), steps + 1);
    assert_eq!(a.editor.history.undo_label(), Some("Trace camera picture"));
    run(&mut a, "edit.undo", json!({})).unwrap();
    assert_eq!(a.editor.doc.len(), 0, "one undo takes the whole trace away");
}

/// With "Delete image after tracing" cleared the picture stays beside the paths; Capture again
/// swaps the picture for a new one.
#[test]
fn trace_from_the_camera_can_keep_the_picture_and_capture_again() {
    let mut a = simulated(320, 240);
    run(&mut a, "camera.start", json!({ "index": 0 })).unwrap();
    crate::capture_trace::open(&mut a).unwrap();
    let mut d = a.images.trace.take().unwrap();
    let first = picture(&a);
    crate::capture_trace::again(&mut a, &mut d).unwrap();
    assert_ne!(picture(&a), first, "a new picture");
    assert_eq!(images(&a), 1, "in place of the old one");
    assert!(a.editor.doc.get(d.ids[0]).is_some(), "the window is on the new picture");
    d.args.delete_image = false;
    crate::capture_trace::finish(&mut a, &d).unwrap();
    assert_eq!((images(&a), paths(&a)), (1, 1), "picture and paths");
    // Capture again needs a camera or picture file to take from.
    let mut a = app();
    a.vision.raw = Some(image::RgbaImage::from_pixel(64, 48, image::Rgba([90, 90, 90, 255])));
    crate::capture_trace::open(&mut a).unwrap();
    let mut d = a.images.trace.take().unwrap();
    assert!(crate::capture_trace::again(&mut a, &mut d).unwrap_err().contains("Start the camera"));
}

/// The camera panel's Trace button opens Trace Image on a fresh picture; Cancel takes it away,
/// Capture again and OK leave only the traced paths [IMG-20].
#[test]
fn the_camera_panel_traces_a_fresh_picture() {
    let mut g = Gui::standard();
    {
        let mut a = g.app();
        a.prefs.opts.capture_system = "simulated".into();
        a.prefs.opts.capture_width = 400;
        a.prefs.opts.capture_height = 300;
        a.vision.prefs.live_enabled = true;
    }
    g.exec("camera.start", json!({ "index": 0 }));
    show_camera_panel(&mut g);
    click_beside(&mut g, "Update overlay", "Trace");
    g.frames(3);
    assert!(g.app().images.trace.is_some(), "Trace Image is open");
    assert!(g.has_text("Capture again"));
    assert_eq!(images(&g.app()), 1);
    g.screenshot("camera_trace_window");
    g.click_label("Cancel");
    g.frames(2);
    assert!(g.app().images.trace.is_none());
    assert_eq!(g.app().editor.doc.len(), 0, "Cancel took the picture away");

    click_beside(&mut g, "Update overlay", "Trace");
    g.frames(3);
    let first = picture(&g.app());
    g.click_label("Capture again");
    g.frames(3);
    assert_ne!(picture(&g.app()), first, "Capture again took a new picture");
    assert_eq!(images(&g.app()), 1);
    g.click_label("OK");
    g.frames(3);
    assert!(g.app().images.trace.is_none(), "{:?}", g.app().images.trace.as_ref().and_then(|d| d.notice.clone()));
    let counts = {
        let a = g.app();
        (images(&a), paths(&a))
    };
    assert_eq!(counts, (0, 1));
    assert_eq!(g.app().editor.history.undo_label(), Some("Trace camera picture"));
    g.screenshot("camera_trace_done");
}

/// Click the `label` button on the row of `anchor`.
fn click_beside(g: &mut Gui, anchor: &str, label: &str) {
    let row = g.find_text(anchor).unwrap_or_else(|| panic!("{anchor} is shown"));
    let r = g
        .find_texts(label)
        .into_iter()
        .filter(|r| (r.center().y - row.center().y).abs() < 4.0)
        .min_by(|a, b| (a.center().x - row.center().x).abs().total_cmp(&(b.center().x - row.center().x).abs()))
        .unwrap_or_else(|| panic!("no {label} button beside {anchor}"));
    g.click_pos(r.center());
}

// ----- Head camera captures at the reported head position [CAM-05] ---------------------------

fn wait(a: &mut DarkBurn, what: &str, mut f: impl FnMut(&DarkBurn) -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        a.machine.tick();
        if f(a) {
            return;
        }
        assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// A head camera capture without a typed position is placed where the connected laser (the
/// simulator here) reports the head; without a laser it asks for the position.
#[test]
fn a_head_capture_is_placed_where_the_laser_reports_the_head() {
    use crate::machine_api::LaserState;
    let mut a = simulated(320, 240);
    run(&mut a, "camera.start", json!({ "index": 1 })).unwrap();
    let e = run(&mut a, "camera.head_add_capture", json!({})).unwrap_err();
    assert!(e.contains("connect the laser"), "{e}");
    assert!(run(&mut a, "camera.head_add_capture", json!({ "x": 5.0 })).unwrap_err().contains("both x and y"));
    let typed = run(&mut a, "camera.head_add_capture", json!({ "x": 12.0, "y": 34.0 })).unwrap();
    assert_eq!((typed["head"]["x"].as_f64(), typed["head"]["y"].as_f64()), (Some(12.0), Some(34.0)), "a typed position counts as typed");

    run(&mut a, "machine.connect", json!({})).unwrap();
    wait(&mut a, "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "machine.move_to", json!({ "x": 100.0, "y": 80.0, "machine_coords": true, "feed": 6000 })).unwrap();
    wait(&mut a, "moved", |a| (a.machine.handle.snapshot().machine_pos[0] - 100.0).abs() < 0.05);
    wait(&mut a, "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    run(&mut a, "machine.get_position", json!({})).unwrap();
    wait(&mut a, "position known", |a| a.machine.handle.snapshot().has_position);
    let r = run(&mut a, "camera.head_add_capture", json!({})).unwrap();
    assert_eq!(r["captures"], 2);
    let (x, y) = (r["head"]["x"].as_f64().unwrap(), r["head"]["y"].as_f64().unwrap());
    assert!((x - 100.0).abs() < 0.05 && (y - 80.0).abs() < 0.05, "the reported head: {r}");
    assert_eq!(a.vision.mosaic.last().map(|c| c.image.dimensions()), Some((320, 240)), "with the latest camera picture");
    assert_eq!(crate::vision_cmds::head_now(&a, None).map(|p| (p.x.round(), p.y.round())), Ok((100.0, 80.0)));
    run(&mut a, "machine.disconnect", json!({})).unwrap();
}

/// Head Camera Alignment: Read head fills in where the laser reports the head, and Add capture
/// uses it [CAM-05].
#[test]
fn the_head_camera_window_reads_the_head_position_from_the_laser() {
    use crate::machine_api::LaserState;
    let mut g = Gui::standard();
    {
        let mut a = g.app();
        a.prefs.opts.capture_system = "simulated".into();
        a.prefs.opts.capture_width = 320;
        a.prefs.opts.capture_height = 240;
        a.vision.prefs.live_enabled = true;
    }
    g.exec("camera.start", json!({ "index": 1 }));
    g.exec("machine.connect", json!({}));
    wait(&mut g.app(), "ready", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    g.exec("machine.move_to", json!({ "x": 100.0, "y": 80.0, "machine_coords": true, "feed": 6000 }));
    wait(&mut g.app(), "moved", |a| (a.machine.handle.snapshot().machine_pos[0] - 100.0).abs() < 0.05);
    wait(&mut g.app(), "idle", |a| a.machine.handle.snapshot().state == LaserState::Ready);
    g.exec("machine.get_position", json!({}));
    wait(&mut g.app(), "position known", |a| a.machine.handle.snapshot().has_position);
    show_camera_panel(&mut g);
    g.click_label("Head camera...");
    g.frames(3);
    g.click_label("Read head");
    g.frames(2);
    g.screenshot("camera_head_read_head");
    // Without the laser the position read stays in the fields and Add capture uses it.
    g.exec("machine.disconnect", json!({}));
    g.frames(2);
    g.click_label("Add capture at head position");
    g.frames(2);
    let a = g.app();
    assert_eq!(a.vision.mosaic.len(), 1, "one capture");
    let want = crate::vision_cmds::laser_to_design(&a, darkburn_core::kurbo::Point::new(100.0, 80.0));
    let got = a.vision.mosaic[0].head_pos;
    assert!((got - want).hypot() < 0.05, "placed at the head the laser reported: {got:?}, want {want:?}");
}
