//! The pictures in the README, made from the real app.
//!
//! Each test drives the app in the headless UI harness (`ui_harness.rs`, docs/UI-TESTING.md),
//! builds a scene with commands, clicks and drags, and writes one PNG into `docs/img/`.
//! Nothing is drawn by hand and no picture file is read: the "photo" is computed here.
//! The machine in the pictures is the built-in GRBL simulator; no real port is ever opened.
//!
//! The tests are ignored, so CI never runs them. To make the pictures again:
//!
//! ```text
//! cargo test -p darkburn readme_shots -- --ignored --test-threads=1
//! ```
//!
//! One thread, because the theme is one setting for the whole process.

use std::path::PathBuf;

use egui::{Pos2, Rect, pos2, vec2};
use serde_json::{Value, json};

use crate::ui_harness::{Gui, outside};

/// The window in points and the pixels per point: every picture is 2880 x 1800 pixels.
const WINDOW: (f32, f32) = (1440.0, 900.0);
const SCALE: f32 = 2.0;

// The layers of the sign, by palette number.
const ENGRAVE: usize = 0;
const CUT: usize = 1;
const SCORE: usize = 2;
const HOLES: usize = 5;
const LIGHT: usize = 9;

fn gui() -> Gui {
    Gui::new(WINDOW.0, WINDOW.1, SCALE)
}

fn out_dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/img")
}

/// Take the pointer off the window, so no button is lit and no pointer is drawn.
fn park(g: &mut Gui) {
    g.hover_pos(pos2(-200.0, -200.0));
    g.frames(2);
}

/// Write the last frame to `docs/img/<name>.png`, after checking nothing went wrong on the way.
fn save(g: &mut Gui, name: &str) {
    let note = g.app().note.clone();
    assert!(note.is_none(), "{name}: the status bar says {note:?}");
    let screen = g.screen();
    for r in g.dialog_windows() {
        assert!(!outside(screen, r), "{name}: the window {r:?} is not inside {screen:?}");
    }
    let rgb = image::DynamicImage::ImageRgba8(g.render_image()).to_rgb8();
    assert_eq!(rgb.dimensions(), ((WINDOW.0 * SCALE) as u32, (WINDOW.1 * SCALE) as u32));
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("docs/img");
    let file = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("{name}.png"))).expect("create the picture"));
    let png =
        image::codecs::png::PngEncoder::new_with_quality(file, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive);
    image::ImageEncoder::write_image(png, rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8).expect("write the picture");
}

fn shot(g: &mut Gui, name: &str) {
    park(g);
    save(g, name);
}

/// Run a command that makes one shape and return the shape's id.
fn add(g: &mut Gui, id: &str, args: Value) -> u64 {
    let r = g.exec(id, args);
    r["id"].as_u64().or_else(|| r["ids"][0].as_u64()).unwrap_or_else(|| panic!("{id} made no shape: {r}"))
}

/// Add a text and put the middle of its `anchor` side at `x`, `y`.
fn text(g: &mut Gui, mut args: Value, anchor: &str, x: f64, y: f64) -> u64 {
    args["x"] = json!(0);
    args["y"] = json!(0);
    let id = add(g, "text.add", args);
    g.exec("shape.place", json!({ "anchor": anchor, "x": x, "y": y }));
    id
}

fn circle(g: &mut Gui, cx: f64, cy: f64, d: f64, layer: usize) -> u64 {
    add(g, "shape.add_ellipse", json!({ "x": cx - d / 2.0, "y": cy - d / 2.0, "width": d, "height": d, "layer": layer }))
}

fn layer(g: &mut Gui, layer: usize, name: &str, mode: &str, speed: f64, max_power: f64, min_power: f64) {
    g.exec("layer.set_mode", json!({ "layer": layer, "mode": mode }));
    for (field, value) in [("name", json!(name)), ("speed", json!(speed)), ("max_power", json!(max_power)), ("min_power", json!(min_power))] {
        g.exec("layer.set_param", json!({ "layer": layer, "field": field, "value": value }));
    }
}

/// Zoom the canvas to this part of the drawing (document millimetres).
fn frame(g: &mut Gui, x0: f64, y0: f64, x1: f64, y1: f64) {
    g.app().view_cmd = Some(crate::canvas::ViewCmd::Frame(darkburn_core::kurbo::Rect::new(x0, y0, x1, y1)));
    g.frames(3);
}

/// Bring a panel's tab to the front: click it, or pick it from the list of tabs that do not fit.
fn show_tab(g: &mut Gui, id: &str) {
    let tab = format!("panel:tab:{id}");
    if g.rect_of(&tab).is_some() {
        g.click_id(&tab);
    } else {
        let area = g.app().prefs.panel(id).area;
        g.click_id(&format!("panel:overflow:{area:?}"));
        let title = crate::panels::PANELS.iter().find(|p| p.id == id).map(|p| crate::i18n::panel_title(p.id, p.title)).expect("a panel");
        g.click_label(&title);
    }
}

/// Drag the window on top by its title bar until its top-left corner is at `to`.
fn move_dialog(g: &mut Gui, to: Pos2) {
    let r = *g.dialog_windows().last().expect("a window is open");
    let grip = pos2(r.center().x, r.top() + 20.0);
    g.drag(grip, grip + (to - r.min));
    g.frames(2);
}

/// Press at `from`, move to `to` and let go, in four frames.
fn quick_drag(g: &mut Gui, from: Pos2, to: Pos2) {
    g.press_pos(from);
    g.hover_pos(to);
    g.release_pos(to);
}

/// The outline of a gear wheel as SVG path data.
fn gear_path(cx: f64, cy: f64, tip: f64, root: f64, teeth: usize) -> String {
    let step = std::f64::consts::TAU / teeth as f64;
    let mut d = String::new();
    for i in 0..teeth {
        for (part, r) in [(0.0, root), (0.18, tip), (0.5, tip), (0.68, root)] {
            let a = (i as f64 + part) * step;
            d += &format!("{}{:.3},{:.3} ", if d.is_empty() { "M" } else { "L" }, cx + r * a.cos(), cy + r * a.sin());
        }
    }
    d + "Z"
}

/// The design most pictures show: a workshop sign, four key tags, a gear and a QR code, on five
/// layers that engrave, score and cut. Returns the id of the gear (a group).
fn sign(g: &mut Gui) -> u64 {
    // The sign: outline, a scored border, four screw holes, the words.
    add(g, "shape.add_rect", json!({ "x": 30, "y": 180, "width": 340, "height": 150, "radius": 12, "layer": CUT }));
    add(g, "shape.add_rect", json!({ "x": 36, "y": 186, "width": 328, "height": 138, "radius": 8, "layer": SCORE }));
    for (x, y) in [(50.0, 200.0), (350.0, 200.0), (50.0, 310.0), (350.0, 310.0)] {
        circle(g, x, y, 7.0, HOLES);
    }
    text(g, json!({ "text": "WORKSHOP", "height": 30, "bold": true, "layer": ENGRAVE }), "centre", 200.0, 268.0);
    add(g, "shape.add_path", json!({ "svg_path": "M95,238 L305,238", "layer": SCORE }));
    text(g, json!({ "text": "MEASURE TWICE \u{b7} CUT ONCE", "height": 9, "letter_spacing": 2, "layer": LIGHT }), "centre", 200.0, 218.0);
    // A gear to cut out.
    let gear = [
        add(g, "shape.add_path", json!({ "svg_path": gear_path(68.0, 128.0, 36.0, 29.0, 14), "layer": CUT })),
        circle(g, 68.0, 128.0, 12.0, HOLES),
        circle(g, 68.0, 128.0, 36.0, SCORE),
    ];
    // Four key tags.
    for (i, name) in ["SHED", "GARAGE", "BIKE", "OFFICE"].into_iter().enumerate() {
        let (x, y) = (122.0 + (i % 2) as f64 * 92.0, 132.0 - (i / 2) as f64 * 38.0);
        add(g, "shape.add_rect", json!({ "x": x, "y": y, "width": 84, "height": 30, "radius": 15, "layer": CUT }));
        circle(g, x + 13.0, y + 15.0, 10.0, HOLES);
        text(g, json!({ "text": name, "height": 9, "bold": true, "layer": LIGHT }), "centre", x + 50.0, y + 15.0);
    }
    add(g, "qr.add", json!({ "x": 340, "y": 128, "text": "Made with DarkBurn", "size": 56, "layer": ENGRAVE }));
    layer(g, SCORE, "Score", "line", 120.0, 12.0, 8.0);
    layer(g, LIGHT, "Engrave light", "fill", 400.0, 16.0, 8.0);
    layer(g, ENGRAVE, "Engrave", "fill", 350.0, 28.0, 10.0);
    layer(g, HOLES, "Holes", "line", 14.0, 65.0, 55.0);
    layer(g, CUT, "Cut out", "line", 12.0, 70.0, 60.0);
    // The order they burn in: score and engrave first, the holes, the outlines last.
    for (l, delta) in [(CUT, 4), (ENGRAVE, 2), (HOLES, 2), (LIGHT, -1)] {
        g.exec("layer.move", json!({ "layer": l, "delta": delta }));
    }
    let order: Vec<usize> = g.app().editor.doc.used_layers();
    assert_eq!(order, [SCORE, LIGHT, ENGRAVE, HOLES, CUT], "the layers are in burning order");
    g.exec("selection.set", json!({ "ids": gear }));
    g.exec("arrange.group", json!({}));
    let group = g.app().editor.selected()[0].0;
    g.exec("selection.clear", json!({}));
    g.exec("layer.select", json!({ "layer": ENGRAVE }));
    g.app().editor.mark_saved();
    frame(g, 45.0, 105.0, 355.0, 320.0);
    group
}

// ----- the pictures -----

/// The main window with the sign, the gear selected.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn hero() {
    let mut g = gui();
    let gear = sign(&mut g);
    g.exec("selection.set", json!({ "ids": [gear] }));
    shot(&mut g, "hero");
}

/// The same in the light theme.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn light() {
    let mut g = gui();
    let gear = sign(&mut g);
    g.exec("setting.set", json!({ "key": "theme", "value": "light" }));
    g.frames(3);
    g.exec("selection.set", json!({ "ids": [gear] }));
    shot(&mut g, "light");
}

/// The picture area of the Preview window.
fn preview_picture(g: &Gui) -> Rect {
    let nodes = g.a11y();
    let readout = nodes.iter().find(|n| n.role == "Label" && n.value.as_deref().is_some_and(|v| v.contains("Operation "))).expect("the readout line");
    // The picture is the unnamed area that starts one row gap below the readout.
    let top = readout.rect.bottom() + 4.0;
    nodes
        .iter()
        .find(|n| n.role == "Unknown" && n.label.is_none() && (n.rect.top() - top).abs() < 3.0 && (n.rect.left() - readout.rect.left()).abs() < 3.0)
        .map(|n| n.rect)
        .expect("the picture of the Preview window")
}

/// The Preview window half-way through the job.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn preview() {
    let mut g = gui();
    sign(&mut g);
    g.run_command("window.preview");
    // The job is planned on a worker thread. Wait for it without drawing frames: the window
    // grows with every frame it is drawn (see the note at the end of this test).
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while g.app().job.pending.as_mut().is_some_and(|(task, _)| task.poll().is_none()) {
        assert!(std::time::Instant::now() < end, "the job was never planned");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    g.frames(2);
    assert!(g.app().job.preview.data.is_some(), "the Preview has its job: {:?}", g.app().job.preview.error);
    // Put the window under the ribbon and make it taller: drag its title bar, then its bottom edge.
    let window = |g: &Gui| g.find_a11y(Some("Window"), "Preview").last().expect("the Preview window").rect;
    let canvas = g.canvas_rect();
    let at = window(&g);
    let grip = pos2(at.left() + 200.0, at.top() + 18.0);
    quick_drag(&mut g, grip, grip + (pos2(canvas.left() + 10.0, canvas.top() - 20.0) - at.min));
    let edge = g.find_a11y(Some("Splitter"), "Resize window bottom edge").last().expect("the window's bottom edge").rect.center();
    quick_drag(&mut g, edge, pos2(edge.x, canvas.bottom()));
    g.hover_pos(pos2(-200.0, -200.0));
    // The window gets wider with every frame it is drawn (see the note at the end). Let it,
    // until it hides the panels on the right: what is drawn is two steps ahead of this
    // rectangle, and has to stay inside the main window. Stop if it ever stops growing.
    const RIGHT: f32 = 1378.0;
    assert!(window(&g).right() < RIGHT, "the Preview window is already {:?}", window(&g));
    let mut growth = 0.0;
    for _ in 0..80 {
        let before = window(&g).right();
        if before >= RIGHT {
            break;
        }
        g.frames(1);
        growth = window(&g).right() - before;
        if growth <= 0.0 {
            break;
        }
    }
    // Zoom to the drawing and stop the clock part-way through the engraving. The frame drawn
    // next is `growth` wider than this one.
    let area = preview_picture(&g);
    let (w, h) = (area.width() + growth, area.height());
    let (design, page_height) = (darkburn_core::kurbo::Rect::new(24.0, 86.0, 376.0, 336.0), f64::from(g.app().prefs.page_height));
    let zoom = (f64::from(w) / design.width()).min(f64::from(h) / design.height());
    {
        let mut app = g.app();
        let p = &mut app.job.preview;
        p.zoom = zoom as f32;
        p.pan = vec2(w / 2.0 - (design.center().x * zoom) as f32, h / 2.0 - ((page_height - design.center().y) * zoom) as f32);
        p.fitted = true;
        p.t = p.data.as_ref().map_or(0.0, |d| d.stats.seconds) * 0.5;
    }
    g.frames(1);
    assert!(g.has_text("Estimated time"), "{:?}", g.visible_texts());
    save(&mut g, "preview");
    // Note: the Preview window is 18 points wider every frame it is drawn (the speed list takes
    // all the width, and the three check boxes after it push the window out), so this picture
    // is taken at a chosen width. Left alone, the window ends up wider than the main window and
    // the check boxes are never seen.
}

/// A grey picture that looks like a photo: a low sun behind mountain ridges and their
/// reflection in a lake. Computed, so the repository holds no photo and nobody's picture is used.
fn landscape(w: u32, h: u32) -> image::GrayImage {
    fn hash(n: i64) -> f64 {
        let mut n = (n as u32).wrapping_mul(0x9E37_79B1);
        n ^= n >> 15;
        n = n.wrapping_mul(0x85EB_CA6B);
        n ^= n >> 13;
        f64::from(n & 0x00FF_FFFF) / 16_777_216.0
    }
    fn smooth(t: f64) -> f64 {
        t * t * (3.0 - 2.0 * t)
    }
    fn noise1(x: f64, seed: i64) -> f64 {
        let i = x.floor();
        let (a, b) = (hash(i as i64 * 131 + seed * 7919), hash((i as i64 + 1) * 131 + seed * 7919));
        a + (b - a) * smooth(x - i)
    }
    fn noise2(x: f64, y: f64, seed: i64) -> f64 {
        let (ix, iy) = (x.floor(), y.floor());
        let (fx, fy) = (smooth(x - ix), smooth(y - iy));
        let at = |a: f64, b: f64| hash(a as i64 * 374_761 + b as i64 * 668_265 + seed * 7919);
        let top = at(ix, iy) + (at(ix + 1.0, iy) - at(ix, iy)) * fx;
        let bottom = at(ix, iy + 1.0) + (at(ix + 1.0, iy + 1.0) - at(ix, iy + 1.0)) * fx;
        top + (bottom - top) * fy
    }
    /// `octaves` layers of noise, each twice as fine and half as strong, 0 to 1.
    fn layered(octaves: i64, f: impl Fn(f64, i64) -> f64) -> f64 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for o in 0..octaves {
            sum += amp * f(freq, o);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
    /// The land above the water: `u` from the left, `v` from the top, both 0 to 1.
    fn land(u: f64, v: f64) -> f64 {
        let to_sun = ((u - 0.66) * 1.5).hypot(v - 0.30);
        let mut sky = if v < 0.62 { 0.42 + 0.50 * (v / 0.62).max(0.0).powf(1.3) } else { 0.92 };
        let cloud = layered(4, |f, o| noise2(u * 5.0 * f, v * 16.0 * f, 40 + o));
        sky -= 0.22 * (cloud - 0.5).max(0.0) * 2.0 * (1.0 - v / 0.5).max(0.0);
        sky += 0.30 * (-to_sun * 7.0).exp();
        if to_sun < 0.058 {
            sky = 1.0;
        }
        let mut value = sky;
        // Ridges from far to near: base line, height, roughness, seed, tone.
        for (base, height, rough, seed, tone) in
            [(0.56, 0.36, 2.0, 11, 0.72), (0.60, 0.34, 2.9, 23, 0.54), (0.63, 0.22, 4.3, 37, 0.34), (0.655, 0.12, 6.0, 51, 0.14)]
        {
            let n = layered(5, |f, o| noise1(u * rough * f, seed + o));
            let crest = base - height * (1.0 - (2.0 * n - 1.0).abs()).powf(1.6) * 0.9;
            if v > crest {
                // Haze under each crest.
                value = tone + 0.10 * (-(v - crest) * 30.0).exp();
            }
        }
        value
    }
    const SHORE: f64 = 0.66;
    image::GrayImage::from_fn(w, h, |x, y| {
        let (u, v) = (f64::from(x) / f64::from(w), f64::from(y) / f64::from(h));
        let value = if v < SHORE {
            land(u, v)
        } else {
            // The lake: the land upside down, darker, with ripples.
            let depth = (v - SHORE) / (1.0 - SHORE);
            let wobble = layered(4, |f, o| noise2(u * 9.0 * f, v * 40.0 * f, 77 + o));
            let ripple = 0.012 * (v * 520.0 + 6.0 * wobble).sin() * (0.3 + depth);
            let grain = layered(4, |f, o| noise2(u * 30.0 * f, v * 160.0 * f, 91 + o));
            land(u + ripple, SHORE - (v - SHORE) * 0.9) * (0.80 - 0.25 * depth) + 0.05 * (grain - 0.5)
        };
        image::Luma([(value.clamp(0.0, 1.0) * 255.0) as u8])
    })
}

/// A photo set up for engraving: dithered on the canvas, with Adjust Image open.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn image_engraving() {
    let mut g = gui();
    let file = g.temp("landscape.png");
    landscape(900, 600).save(&file).expect("write the photo");
    let r = g.exec("project.import", json!({ "paths": [file.to_string_lossy()], "x": 200, "y": 200 }));
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    layer(&mut g, 0, "Photo", "image", 250.0, 35.0, 0.0);
    g.exec("image.adjust", json!({ "image_mode": "jarvis", "dpi": 169, "contrast": 10, "gamma": 1.1, "enhance_amount": 60, "enhance_radius": 2.0 }));
    for name in ["Birch plywood 3 mm", "Slate coaster"] {
        g.exec("image.preset_save", json!({ "name": name }));
    }
    g.app().editor.mark_saved();
    // The photo is 90 x 60 mm about (200, 200). Zoom in on it and keep it on the left, where
    // the window will not cover it.
    let zoom = 6.2;
    let canvas = g.canvas_rect();
    g.app().prefs.view.zoom = zoom;
    g.app().prefs.view.offset = vec2(28.0 - 155.0 * zoom, (canvas.height() - 60.0 * zoom) / 2.0 - 170.0 * zoom);
    g.frames(2);
    g.run_command("tools.adjust_image");
    g.settle();
    assert!(g.window_titles().contains(&"Adjust Image".to_string()), "{:?}", g.window_titles());
    let size = g.dialog_windows().last().expect("the Adjust Image window").size();
    let screen = g.screen();
    move_dialog(&mut g, pos2(screen.right() - size.x - 6.0, 0.0));
    // The dots on the canvas and in the window are worked out on worker threads.
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        g.frames(1);
    }
    shot(&mut g, "image-engraving");
}

/// The Cut Settings Editor of the engraving layer, opened by a double click on its row.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn cut_settings() {
    let mut g = gui();
    sign(&mut g);
    g.double_click_label("00");
    g.settle();
    assert_eq!(g.app().cut_settings_open, Some(ENGRAVE));
    let size = g.dialog_windows().last().expect("the Cut Settings Editor").size();
    let screen = g.screen();
    let left = screen.right() - size.x - 6.0;
    move_dialog(&mut g, pos2(left, 0.0));
    // Show the whole sign in the part of the canvas the window leaves free.
    let canvas = g.canvas_rect();
    let (zoom, page_height) = (1.72, g.app().prefs.page_height);
    g.app().prefs.view.zoom = zoom;
    g.app().prefs.view.offset = vec2((left - canvas.left()) / 2.0 - 200.0 * zoom, canvas.height() / 2.0 - (page_height - 212.0) * zoom);
    g.frames(2);
    shot(&mut g, "cut-settings");
}

/// Shape tools: a circular array with an offset rim, a part made with booleans, a grid array
/// and a curve in the node editor.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn geometry() {
    let mut g = gui();
    // A coaster: one petal copied round a centre, a rim, and the rim offset outwards.
    let petal = "M100,269 C113,284.4 113,302 100,313 C87,302 87,284.4 100,269 Z";
    add(&mut g, "shape.add_path", json!({ "svg_path": petal, "layer": SCORE }));
    g.exec("arrange.circular_array", json!({ "cx": 100, "cy": 255, "count": 12, "rotate_items": true, "virtual": true }));
    circle(&mut g, 100.0, 255.0, 128.0, CUT);
    g.exec("tools.offset", json!({ "distance": 5, "side": "outward" }));
    // A bottle opener: handle and head joined, two holes taken out, then an outline round it.
    let handle = add(&mut g, "shape.add_rect", json!({ "x": 195, "y": 233, "width": 130, "height": 44, "radius": 22, "layer": CUT }));
    let head = circle(&mut g, 335.0, 255.0, 90.0, CUT);
    g.exec("selection.set", json!({ "ids": [handle, head] }));
    let body = add(&mut g, "tools.union", json!({}));
    let holes = [circle(&mut g, 335.0, 255.0, 48.0, CUT), circle(&mut g, 216.0, 255.0, 16.0, CUT)];
    g.exec("selection.set", json!({ "ids": [body, holes[0], holes[1]] }));
    let opener = add(&mut g, "tools.difference", json!({}));
    g.exec("selection.set", json!({ "ids": [opener] }));
    g.exec("tools.offset", json!({ "distance": 4, "side": "outward" }));
    // A honeycomb: one hexagon in a grid array, every other row shifted.
    add(&mut g, "shape.add_polygon", json!({ "cx": 250, "cy": 132, "sides": 6, "radius": 13, "layer": 3 }));
    g.exec("arrange.grid_array", json!({ "rows": 3, "cols": 5, "dx": 3, "dy": 3, "shift": "hex", "virtual": true }));
    // A curve, opened in the node editor with one node picked so its handles show.
    let curve = add(&mut g, "shape.add_path", json!({ "svg_path": "M30,95 C55,150 95,60 125,105 S180,150 215,90", "layer": HOLES }));
    for (l, name, speed, power) in [(CUT, "Cut", 12.0, 70.0), (SCORE, "Score", 120.0, 12.0), (3, "Mark", 200.0, 18.0), (HOLES, "Sketch", 150.0, 10.0)]
    {
        layer(&mut g, l, name, "line", speed, power, power - 5.0);
    }
    g.app().editor.mark_saved();
    frame(&mut g, 45.0, 75.0, 365.0, 315.0);
    g.click_id("ribbon:tab:modify");
    g.exec("selection.set", json!({ "ids": [curve] }));
    g.run_command("tools.nodes");
    // The node at (125, 105): the canvas takes page millimetres, Y down.
    let page_height = g.app().prefs.page_height;
    g.click_mm(125.0, page_height - 105.0);
    shot(&mut g, "geometry");
}

/// A job running on the built-in GRBL simulator, with the console and the jog panel.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn machines() {
    use crate::machine_api::LaserState;
    let mut g = gui();
    sign(&mut g);
    // The Move panel in a dock of its own on the left.
    {
        let mut app = g.app();
        let p = app.prefs.panels.get_mut("move").expect("the Move panel");
        p.area = crate::panels::Area::Left;
        p.visible = true;
        app.prefs.dock_sizes.insert("left".into(), 330.0);
    }
    g.frames(3);
    assert!(g.app().machine.active().is_simulator(), "the pictures only ever use a simulator");
    g.run_command("machine.connect");
    let wait = |g: &mut Gui, what: &str, done: &dyn Fn(&crate::machine_api::Snapshot) -> bool| {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while !done(g.app().machine.handle.snapshot()) {
            assert!(std::time::Instant::now() < end, "the simulator never got to: {what}");
            std::thread::sleep(std::time::Duration::from_millis(20));
            g.frames(1);
        }
    };
    wait(&mut g, "connected", &|s| s.state == LaserState::Ready);
    show_tab(&mut g, "console");
    g.exec("machine.console", json!({ "line": "$I" }));
    g.exec("machine.show_all", json!({ "on": true }));
    g.frames(4);
    frame(&mut g, 45.0, 105.0, 355.0, 320.0);
    // Cut the parts out: only the shapes of the cutting layer, so the job is a short one.
    g.exec("layer.select_shapes", json!({ "layer": CUT }));
    g.exec("job.set_origin", json!({ "cut_selected": true }));
    g.exec("job.start", json!({}));
    g.frames(4);
    g.exec("selection.clear", json!({}));
    wait(&mut g, "a third of the job", &|s| s.job.as_ref().and_then(|j| j.fraction).is_some_and(|f| f >= 0.36));
    shot(&mut g, "machines");
    g.exec("job.stop", json!({}));
    g.frames(3);
}

/// Variable text, a barcode, a Data Matrix and a QR code, with the Text tab and the Variable
/// Text panel in front.
#[test]
#[ignore = "writes a README picture: cargo test -p darkburn readme_shots -- --ignored --test-threads=1"]
fn variable_text() {
    let mut g = gui();
    add(&mut g, "shape.add_rect", json!({ "x": 40, "y": 140, "width": 320, "height": 170, "radius": 8, "layer": CUT }));
    for (x, y) in [(54.0, 154.0), (346.0, 154.0), (54.0, 296.0), (346.0, 296.0)] {
        circle(&mut g, x, y, 8.0, CUT);
    }
    text(&mut g, json!({ "text": "WORKSHOP ASSET", "height": 15, "bold": true, "layer": ENGRAVE }), "left", 72.0, 284.0);
    add(&mut g, "shape.add_path", json!({ "svg_path": "M72,268 L328,268", "layer": SCORE }));
    let mono = "IBM Plex Mono";
    let serial = text(
        &mut g,
        json!({ "text": "Tag no.  {serial:4}", "height": 9, "variable": "serial", "font": mono, "layer": ENGRAVE }),
        "left",
        72.0,
        252.0,
    );
    text(&mut g, json!({ "text": "Checked  %d.%m.%Y", "height": 9, "variable": "date_time", "font": mono, "layer": ENGRAVE }), "left", 72.0, 234.0);
    text(
        &mut g,
        json!({ "text": "Engraved at {cut:S} mm/s, {cut:P} %", "height": 5, "variable": "cut_setting", "layer": ENGRAVE }),
        "left",
        72.0,
        219.0,
    );
    g.exec("variable.set", json!({ "current": 42, "padding": 4 }));
    let code = "WS-{serial:4}";
    add(
        &mut g,
        "barcode.add",
        json!({ "x": 118, "y": 180, "barcode": "code128", "data": code, "variable": true, "module": 0.6, "height": 26, "layer": ENGRAVE }),
    );
    add(
        &mut g,
        "barcode.add",
        json!({ "x": 218, "y": 180, "barcode": "datamatrix", "data": code, "variable": true, "module": 1.8, "layer": ENGRAVE }),
    );
    add(&mut g, "qr.add", json!({ "x": 296, "y": 196, "text": "Workshop asset {serial:4}", "variable": true, "size": 64, "layer": ENGRAVE }));
    layer(&mut g, ENGRAVE, "Engrave", "fill", 300.0, 25.0, 10.0);
    layer(&mut g, SCORE, "Score", "line", 120.0, 12.0, 8.0);
    layer(&mut g, CUT, "Cut out", "line", 12.0, 70.0, 60.0);
    g.exec("layer.move", json!({ "layer": CUT, "delta": 2 }));
    g.app().editor.mark_saved();
    frame(&mut g, 60.0, 145.0, 340.0, 305.0);
    g.click_id("ribbon:tab:text");
    show_tab(&mut g, "variable_text");
    g.exec("selection.set", json!({ "ids": [serial] }));
    shot(&mut g, "variable-text");
}
