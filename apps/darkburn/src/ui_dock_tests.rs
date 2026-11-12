//! Windows that stay open while the user works: each one docks, floats and opens in a window of
//! its own through the window kit, keeps its edges in reach, comes back where it was and keeps
//! showing what is going on wherever it is.

use darkburn_ui::titlebar;
use darkburn_ui::ui_kit::window_host::{self, Placement, WindowPlace};
use egui::{Rect, pos2, vec2};

use crate::commands;
use crate::ui_harness::Gui;

const OFFSET: &str = "Offset Shapes";
const ALIGN: &str = "Calibrate Camera Alignment";

fn window(g: &Gui, title: &str) -> Rect {
    g.find_a11y(Some("Window"), title).last().unwrap_or_else(|| panic!("the {title} window is open: {:?}", g.window_titles())).rect
}

fn placement(g: &Gui, key: &str) -> Placement {
    g.app().prefs.windows.get(key).map(|p| p.placement.clone()).unwrap_or_default()
}

/// The main window's own resize zones (the custom title bar's), which take the pointer first.
fn main_resize_zones(g: &Gui) -> Vec<Rect> {
    let edge = titlebar::resize_border_px(g.scale()) / g.scale();
    titlebar::zones(g.screen(), edge).iter().map(|z| z.0).collect()
}

fn assert_clear_of_main_border(g: &Gui, what: &str, r: Rect) {
    for zone in main_resize_zones(g) {
        assert!(!zone.intersects(r.shrink(0.5)), "{what} {r:?} is under the main window's resize border {zone:?}");
    }
}

fn open_preview(g: &mut Gui) {
    g.sample_design();
    g.run_command("window.preview");
    g.wait_for_job_build();
    g.frames(4);
    assert!(g.app().job.preview.data.is_some(), "the preview has a job: {:?}", g.app().job.preview.error);
}

fn preview_clock(g: &Gui) -> Option<String> {
    g.visible_texts().into_iter().find(|t| t.contains(" / ") && t.chars().any(|c| c.is_ascii_digit()))
}

// ----- the bottom corner -----

#[test]
fn a_floating_window_dragged_to_the_bottom_of_the_screen_resizes_by_its_bottom_corner() {
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.frames(3);
    let win = g.rect_of("dialog:window").expect("the Settings window");
    // Dragged down by its header as far as it goes: the pointer ends over the status bar.
    let to = pos2(g.canvas_rect().center().x, g.screen().bottom() - 6.0);
    g.drag(pos2(win.left() + 120.0, win.top() + 12.0), to);
    assert_eq!(placement(&g, "settings"), Placement::Floating, "not docked by the drop");
    g.frames(2);
    let before = window(&g, "Settings");
    assert!(before.bottom() > g.screen().bottom() - 60.0, "at the bottom: {before:?} in {:?}", g.screen());
    let corner = g.rect_of("dialog:resize:corner").expect("a resize corner");
    for edge in ["corner", "right", "bottom"] {
        assert_clear_of_main_border(&g, edge, g.rect_of(&format!("dialog:resize:{edge}")).unwrap());
    }
    g.screenshot("dock-settings-at-the-bottom");

    // Its bottom corner takes the drag: smaller both ways.
    g.drag(corner.center(), corner.center() + vec2(-80.0, -60.0));
    g.frames(2);
    let after = window(&g, "Settings");
    assert!(after.width() < before.width() - 40.0 && after.height() < before.height() - 30.0, "{before:?} -> {after:?}");
    assert!(g.app().prefs.windows["settings"].size.is_some(), "the size is remembered");
    // And back out again by the same corner, down to where it was.
    let corner = g.rect_of("dialog:resize:corner").unwrap();
    g.drag(corner.center(), corner.center() + vec2(60.0, 50.0));
    g.frames(2);
    let again = window(&g, "Settings");
    assert!(again.height() > after.height() + 30.0, "{after:?} -> {again:?}");
    assert_clear_of_main_border(&g, "corner", g.rect_of("dialog:resize:corner").unwrap());
}

#[test]
fn the_floating_preview_resizes_by_its_bottom_corner_in_a_short_window() {
    // Short enough that the Preview reaches the bottom of the screen.
    let mut g = Gui::new(1280.0, 640.0, 1.0);
    open_preview(&mut g);
    let before = window(&g, "Preview");
    assert!(before.bottom() > g.screen().bottom() - 60.0, "{before:?} in {:?}", g.screen());
    let corner = Rect::from_min_max(before.max - vec2(10.0, 10.0), before.max);
    assert_clear_of_main_border(&g, "the Preview's corner", corner);
    g.screenshot("dock-preview-at-the-bottom");
    g.drag(before.max - vec2(3.0, 3.0), before.max - vec2(103.0, 83.0));
    g.frames(2);
    let after = window(&g, "Preview");
    assert!(after.width() < before.width() - 60.0 && after.height() < before.height() - 50.0, "{before:?} -> {after:?}");
}

// ----- stored places -----

#[test]
fn a_window_stored_off_the_screen_opens_on_it() {
    let mut prefs = crate::prefs::Prefs::default();
    prefs.windows.insert("settings".into(), WindowPlace { pos: Some([5000.0, 5000.0]), ..Default::default() });
    prefs.windows.insert("db_shape_offset_shapes".into(), WindowPlace { pos: Some([-3000.0, -900.0]), ..Default::default() });
    let mut g = Gui::with_prefs(1280.0, 720.0, 1.0, prefs);
    g.app().settings_open = true;
    g.run_command("tools.offset");
    g.frames(3);
    let screen = g.screen();
    for title in ["Settings", OFFSET] {
        let r = window(&g, title);
        assert!(screen.contains_rect(r), "{title} {r:?} inside {screen:?}");
        assert_clear_of_main_border(&g, title, r);
    }
    g.screenshot("dock-stored-off-the-screen");
}

#[test]
fn windows_docked_together_keep_their_tab_order_after_a_restart() {
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.run_command("tools.offset");
    g.frames(2);
    // Docked Settings first, then Offset Shapes: not the order of their keys.
    let ctx = g.ctx();
    window_host::update(&ctx, "settings", |p| p.dock("RightTop"));
    g.frames(2);
    window_host::update(&ctx, "db_shape_offset_shapes", |p| p.dock("RightTop"));
    g.frames(3);
    // The tabs in the order the area draws them (some may sit in its overflow list).
    let order = |g: &Gui| window_host::docked_tabs(&g.ctx(), "RightTop").into_iter().map(|t| t.key).collect::<Vec<_>>();
    let want = ["settings", "db_shape_offset_shapes"];
    assert_eq!(order(&g), want, "open: {:?}", window_host::open_windows(&g.ctx()));
    // Where both tabs are drawn, Settings comes first.
    if let (Some(a), Some(b)) = (g.rect_of("panel:tab:window:settings"), g.rect_of("panel:tab:window:db_shape_offset_shapes")) {
        assert!(a.left() < b.left() || a.top() < b.top(), "{a:?} before {b:?}");
    }
    g.screenshot("dock-two-tabs");

    // Saved, read back and opened again: the same order.
    let mut prefs: crate::prefs::Prefs = serde_json::from_str(&g.app().prefs.to_json()).unwrap();
    prefs.sanitise();
    let mut again = Gui::with_prefs(1600.0, 1000.0, 1.0, prefs);
    again.app().settings_open = true;
    again.run_command("tools.offset");
    again.frames(3);
    assert_eq!(placement(&again, "settings"), Placement::Docked("RightTop".into()));
    assert_eq!(order(&again), want);
}

#[test]
fn reset_to_default_layout_floats_every_window_at_its_first_place() {
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.run_command("tools.offset");
    open_preview(&mut g);
    g.frames(2);
    let first = |g: &Gui| ["Settings", OFFSET, "Preview"].map(|t| window(g, t));
    let home = first(&g);

    let ctx = g.ctx();
    window_host::update(&ctx, "settings", |p| p.dock("RightTop"));
    window_host::update(&ctx, "db_shape_offset_shapes", |p| p.detach());
    window_host::update(&ctx, crate::job_ui::PREVIEW_KEY, |p| p.dock("RightTop"));
    g.frames(4);
    assert!(!g.window_titles().iter().any(|t| t == "Settings" || t == "Preview"), "docked: {:?}", g.window_titles());

    g.run_command("window.reset_layout");
    g.frames(3);
    for key in ["settings", "db_shape_offset_shapes", crate::job_ui::PREVIEW_KEY] {
        assert_eq!(placement(&g, key), Placement::Floating, "{key}");
    }
    assert_eq!(first(&g), home, "back where they first opened, at their first sizes");
    g.screenshot("dock-after-layout-reset");

    // The Preview the user moved and maximised goes back too.
    let p = window(&g, "Preview");
    g.drag(p.left_top() + vec2(200.0, 12.0), p.left_top() + vec2(320.0, 80.0));
    g.app().job.preview.maximised = true;
    g.frames(3);
    g.run_command("window.reset_layout");
    g.frames(3);
    assert_eq!(window(&g, "Preview"), home[2]);
}

// ----- a window of its own -----

#[test]
fn esc_and_close_in_a_window_of_its_own_close_it_as_when_floating() {
    let mut prefs = crate::prefs::Prefs::default();
    prefs.windows.insert("db_shape_offset_shapes".into(), WindowPlace { placement: Placement::Detached, ..Default::default() });
    let mut g = Gui::with_prefs(1600.0, 1000.0, 1.0, prefs);
    g.run_command("tools.offset");
    g.frames(2);
    assert!(g.window_titles().iter().any(|t| t == OFFSET), "{:?}", g.window_titles());
    g.screenshot("dock-offset-detached");
    g.escape();
    g.frames(2);
    assert_eq!(g.app().shape_ui.open, None, "Esc closed it");
    assert!(!g.window_titles().iter().any(|t| t == OFFSET), "{:?}", g.window_titles());

    g.run_command("tools.offset");
    g.frames(2);
    g.click_a11y(None, "Close");
    g.frames(2);
    assert_eq!(g.app().shape_ui.open, None, "its close button closed it");
    assert_eq!(placement(&g, "db_shape_offset_shapes"), Placement::Detached, "it opens on its own again next time");
}

// ----- live contents -----

/// Pixels in `r` close to `rgb`.
fn pixels_like(img: &image::RgbaImage, r: Rect, rgb: [u8; 3]) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (x0, y0) = (r.left().clamp(0.0, w) as u32, r.top().clamp(0.0, h) as u32);
    let (x1, y1) = (r.right().clamp(0.0, w) as u32, r.bottom().clamp(0.0, h) as u32);
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y).0;
            if (0..3).all(|i| p[i].abs_diff(rgb[i]) < 40) {
                n += 1;
            }
        }
    }
    n
}

fn camera_picture(g: &mut Gui, rgb: [u8; 3]) {
    let img = image::RgbaImage::from_pixel(64, 48, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    crate::vision_cmds::set_frame(&mut g.app(), img).expect("a camera picture");
}

/// The camera picture in the alignment window changes as the camera's does, in `r`.
fn assert_picture_follows_the_camera(g: &mut Gui, what: &str, r: impl Fn(&Gui) -> Rect) {
    const RED: [u8; 3] = [230, 20, 20];
    const BLUE: [u8; 3] = [20, 40, 230];
    camera_picture(g, RED);
    g.frames(2);
    let img = g.render_image();
    let red = pixels_like(&img, r(g), RED);
    assert!(red > 2000, "{what}: the red picture shows ({red} pixels)");
    camera_picture(g, BLUE);
    g.frames(2);
    let img = g.render_image();
    let (blue, red) = (pixels_like(&img, r(g), BLUE), pixels_like(&img, r(g), RED));
    assert!(blue > 2000 && red < 100, "{what}: the blue picture took its place ({blue} blue, {red} red)");
}

#[test]
fn the_camera_picture_stays_live_docked_and_in_a_window_of_its_own() {
    let mut g = Gui::standard();
    g.run_command("laser.camera_alignment");
    g.frames(2);
    assert!(g.window_titles().iter().any(|t| t == ALIGN), "{:?}", g.window_titles());
    let key = crate::vision_ui::dock_key(ALIGN);
    let ctx = g.ctx();
    window_host::update(&ctx, &key, |p| p.dock("RightTop"));
    g.frames(3);
    assert!(g.rect_of(&format!("panel:tab:window:{key}")).is_some(), "docked: {:?}", g.rects("panel:tab:"));
    assert_picture_follows_the_camera(&mut g, "docked", |g| g.rect_of("panel:area:RightTop").unwrap());
    g.screenshot("dock-camera-alignment-docked");

    window_host::update(&ctx, &key, |p| p.detach());
    g.frames(3);
    assert_picture_follows_the_camera(&mut g, "its own window", |g| window(g, ALIGN));
    g.screenshot("dock-camera-alignment-detached");
}

#[test]
fn the_preview_keeps_playing_docked_and_in_a_window_of_its_own() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let ctx = g.ctx();
    for (what, place) in [("docked", Placement::Docked("RightTop".into())), ("its own window", Placement::Detached)] {
        window_host::update(&ctx, crate::job_ui::PREVIEW_KEY, |p| match &place {
            Placement::Docked(area) => p.dock(area),
            _ => p.detach(),
        });
        g.frames(3);
        assert_eq!(placement(&g, crate::job_ui::PREVIEW_KEY), place);
        // It plays on while it is there.
        {
            let mut app = g.app();
            let p = &mut app.job.preview;
            p.t = 0.0;
            p.playing = true;
        }
        g.frames(6);
        let t = g.app().job.preview.t;
        assert!(t > 0.0, "{what}: the Preview plays");
        g.app().job.preview.playing = false;
        g.app().job.preview.t = 0.0;
        g.frames(1);
        let start = preview_clock(&g).unwrap_or_else(|| panic!("{what}: the Preview shows its clock: {:?}", g.visible_texts()));
        // And shows the time the job has got to, with no input in between.
        let total = g.app().job.preview.data.as_ref().map_or(0.0, |d| d.stats.seconds);
        g.app().job.preview.t = total;
        g.frames(1);
        let now = preview_clock(&g).unwrap();
        assert_ne!(now, start, "{what}: the clock follows the job");
        g.app().job.preview.playing = false;
        g.screenshot(&format!("dock-preview-{}", if what == "docked" { "docked" } else { "detached" }));
    }
}

// ----- every window that stays open goes through the kit -----

/// Windows that open for one answer and close with it: they float above everything, are not
/// worked beside, and so are not docked.
const ONE_SHOT: &[&str] = &[
    // Read once and closed.
    "About DarkBurn",
    "Check for Updates",
    // A message and its one button.
    "Could not open",
    "No job",
    "Things to know about this file",
    // Asked before going on: answered, the step goes on.
    "Save changes?",
    // Set up, then OK (or Cancel) does it once and closes the window.
    "Convert to Bitmap",
    "Create QR Code",
    "Export",
    "Export Gweike DXF",
    "Export Gweike G-code",
    "Print",
    "Macro 1",
    // Picks a machine or walks through setting one up, step by step, then closes.
    "Find my laser",
    "New device: Type",
    "New device: Connection",
    "New device: Name and size",
    "New device: Origin and homing",
    "New device: Summary",
];

fn fresh_design(g: &mut Gui) {
    g.close_dialogs();
    crate::files::new_project(&mut g.app());
    g.sample_design();
}

/// Each window open now: a dockable kit window, or one of the one-shot windows.
fn unwrapped(g: &Gui, what: &str, out: &mut std::collections::BTreeSet<String>) {
    let hosted: Vec<String> = window_host::open_windows(&g.ctx()).into_iter().map(|(_, title)| title).collect();
    for title in g.window_titles() {
        if !hosted.contains(&title) && !ONE_SHOT.contains(&title.as_str()) {
            out.insert(format!("{title:?} (from {what})"));
        }
    }
}

#[test]
fn every_window_that_stays_open_docks() {
    let mut g = Gui::standard();
    let mut out = std::collections::BTreeSet::new();
    let ids: Vec<&str> = commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    for id in ids {
        fresh_design(&mut g);
        g.run_command(id);
        g.wait_for_job_build();
        g.frames(2);
        unwrapped(&g, id, &mut out);
    }
    for (what, open) in crate::ui_smoke_tests::state_dialog_cases() {
        fresh_design(&mut g);
        open(&mut g.app());
        g.frames(3);
        unwrapped(&g, what, &mut out);
    }
    g.close_dialogs();
    assert!(out.is_empty(), "windows that stay open but are not dockable kit windows: {out:#?}");
}

#[test]
fn zz_debug_escape_after_cancel() {
    let mut g = Gui::standard();
    g.sample_design();
    g.run_command("tools.offset");
    g.frames(2);
    let dump = |g: &Gui, at: &str| {
        let ctx = g.ctx();
        let layers = ctx.memory(|m| m.areas().visible_layer_ids());
        let id = egui::Id::unique(("shape_dialog", OFFSET));
        let area = egui::AreaState::load(&ctx, id).map(|a| a.rect());
        eprintln!("DBG {at}: place {:?} open {:?}", window_host::place(&ctx, &crate::shape_ui::dock_key(OFFSET)), window_host::open_windows(&ctx));
        eprintln!(
            "DBG {at}: window {id:?} area {area:?} layers {layers:?} titles {:?} popup {} only {}",
            g.window_titles(),
            egui::Popup::is_any_open(&ctx),
            crate::escape::only_window(&ctx, id)
        );
    };
    dump(&g, "first");
    g.click_label("Cancel");
    g.run_command("tools.offset");
    g.frames(2);
    dump(&g, "again");
    g.escape();
    eprintln!("DBG open after Esc: {:?}", g.app().shape_ui.open);
    panic!("debug");
}
