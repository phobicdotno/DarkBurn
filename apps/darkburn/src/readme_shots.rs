//! TEMPORARY explorer: runs a JSON script against the UI harness. Replaced by the real scenes.
use egui::{Key, Modifiers, pos2, vec2};
use serde_json::{Value, json};

use crate::ui_harness::Gui;

fn p2(v: &Value) -> egui::Pos2 {
    pos2(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32)
}

fn key_of(s: &str) -> Key {
    Key::from_name(s).unwrap_or_else(|| panic!("unknown key {s}"))
}

fn mods_of(s: &str) -> Modifiers {
    let mut m = Modifiers::NONE;
    for part in s.split('|') {
        match part {
            "ctrl" => m |= Modifiers::COMMAND,
            "shift" => m |= Modifiers::SHIFT,
            "alt" => m |= Modifiers::ALT,
            "" => {}
            o => panic!("unknown modifier {o}"),
        }
    }
    m
}

/// A soft grey picture that looks like a photo: a lit sphere on a gradient with a shadow.
fn photo(w: u32, h: u32) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let mut v = 0.25 + 0.55 * fy + 0.1 * (fx * 3.0).sin();
        // Ground shadow.
        let (sx, sy) = ((fx - 0.55) / 0.33, (fy - 0.8) / 0.07);
        let s = sx * sx + sy * sy;
        if s < 1.0 {
            v *= 0.45 + 0.55 * s;
        }
        // The sphere.
        let (cx, cy, r) = (0.5, 0.47, 0.33);
        let (dx, dy) = ((fx - cx) / r, (fy - cy) / r * (h as f32 / w as f32));
        let d2 = dx * dx + dy * dy;
        if d2 < 1.0 {
            let dz = (1.0 - d2).sqrt();
            let light = (-0.5 * dx - 0.6 * dy + 0.62 * dz).max(0.0);
            let spec = light.powf(24.0);
            v = 0.08 + 0.75 * light + 0.5 * spec;
        }
        let g = (v.clamp(0.0, 1.0) * 255.0) as u8;
        image::Rgb([g, g, g])
    })
}

fn step(g: &mut Gui, op: &Value, out: &std::path::Path) {
    let (name, v) = op.as_object().and_then(|o| o.iter().next()).expect("an op is {name: value}");
    match name.as_str() {
        "exec" => {
            let ctx = g.ctx();
            let args = v.get(1).cloned().unwrap_or(json!({}));
            let r = crate::api::execute(&mut g.app(), &ctx, v[0].as_str().unwrap(), &args);
            g.frames(2);
            println!("exec {} -> {:?}", v[0], r.map(|x| x.to_string().chars().take(400).collect::<String>()));
        }
        "run" => g.run_command(v.as_str().unwrap()),
        "click_label" => {
            g.click_label(v.as_str().unwrap());
        }
        "click_label_nth" => {
            g.click_label_nth(v[0].as_str().unwrap(), v[1].as_u64().unwrap() as usize);
        }
        "click_id" => {
            g.click_id(v.as_str().unwrap());
        }
        "click_pos" => {
            g.click_pos(p2(v));
        }
        "right_click_pos" => {
            g.right_click_pos(p2(v));
        }
        "click_mm" => {
            g.click_mm(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32);
        }
        "dclick_label" => {
            g.double_click_label(v.as_str().unwrap());
        }
        "dclick_pos" => {
            g.double_click_pos(p2(v));
        }
        "dclick_mm" => {
            let p = g.mm_to_screen(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32);
            g.double_click_pos(p);
        }
        "click_a11y" => {
            g.click_a11y(v[0].as_str(), v[1].as_str().unwrap());
        }
        "drag_mm" => {
            let (a, b) = (p2(&v[0]), p2(&v[1]));
            g.drag_mm((a.x, a.y), (b.x, b.y));
        }
        "drag" => {
            g.drag(p2(&v[0]), p2(&v[1]));
        }
        "press_mm" => {
            let p = g.mm_to_screen(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32);
            g.press_pos(p);
        }
        "hover" => {
            g.hover_pos(p2(v));
        }
        "hover_mm" => {
            let p = g.mm_to_screen(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32);
            g.hover_pos(p);
        }
        "hover_label" => {
            let r = g.find_text(v.as_str().unwrap()).expect("label");
            g.hover_pos(r.center());
        }
        "scroll" => {
            g.scroll(p2(&v[0]), vec2(0.0, v[1].as_f64().unwrap() as f32));
        }
        "menu" => {
            g.open_menu(v[0].as_str().unwrap(), v[1].as_str().unwrap());
        }
        "menu_only" => {
            g.open_menu_only(v.as_str().unwrap());
        }
        "key" => {
            g.key(key_of(v[0].as_str().unwrap()), mods_of(v[1].as_str().unwrap_or("")));
        }
        "type" => {
            g.type_text(v.as_str().unwrap());
        }
        "frames" => {
            g.frames(v.as_u64().unwrap() as usize);
        }
        "settle" => {
            g.settle();
        }
        "sleep" => std::thread::sleep(std::time::Duration::from_millis(v.as_u64().unwrap())),
        "close_dialogs" => {
            g.close_dialogs();
        }
        "shot" => {
            let t = std::time::Instant::now();
            let img = g.render_image();
            let path = out.join(format!("{}.png", v.as_str().unwrap()));
            img.save(&path).unwrap();
            println!("shot {} {}x{} in {:?}", path.display(), img.width(), img.height(), t.elapsed());
        }
        "dump" => match v.as_str().unwrap() {
            "texts" => println!("texts: {:?}", g.painted().iter().map(|p| (p.text.clone(), p.rect.min.x as i32, p.rect.min.y as i32)).collect::<Vec<_>>()),
            "a11y" => {
                for n in g.a11y() {
                    println!("a11y: {} {:?} {:?} {:?}", n.role, n.label, n.value, n.rect);
                }
            }
            "windows" => println!("windows: {:?} {:?}", g.window_titles(), g.dialog_windows()),
            "problems" => println!("problems: {:#?}", g.layout_problems()),
            "info" => {
                let ctx = g.ctx();
                let r = crate::api::execute(&mut g.app(), &ctx, "project.info", &json!({}));
                println!("info: {}", r.unwrap());
            }
            "canvas" => println!("canvas: {:?} view {:?} screen {:?}", g.canvas_rect(), g.app().prefs.view, g.screen()),
            "devices" => {
                let app = g.app();
                for d in &app.machine.devices.devices {
                    println!("device: {} {:?} {:?}", d.id, d.name, d.kind);
                }
                println!("active: {:?} snapshot state {:?}", app.machine.devices.active, app.machine.handle.snapshot().state);
            }
            "note" => println!("note: {:?} hint {:?}", g.app().note, g.app().hint),
            p => {
                for (n, r) in g.rects(p) {
                    println!("rect: {n} {r:?}");
                }
            }
        },
        "wait" => {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let done = match v.as_str().unwrap() {
                    "preview" => g.app().job.preview.data.is_some(),
                    "ready" => g.app().machine.handle.snapshot().state == crate::machine_api::LaserState::Ready,
                    "job_build" => g.app().job.pending.is_none(),
                    "grbl_settings" => !g.app().machine.handle.snapshot().grbl_settings.is_empty(),
                    o => panic!("unknown wait {o}"),
                };
                if done {
                    break;
                }
                assert!(std::time::Instant::now() < end, "waited too long for {v}");
                std::thread::sleep(std::time::Duration::from_millis(5));
                g.frames(1);
            }
        }
        "theme" => {
            g.app().prefs.theme = v.as_str().unwrap().to_string();
            g.frames(3);
        }
        "ribbon_tab" => {
            g.app().prefs.ribbon_tab = v.as_str().unwrap().to_string();
            g.frames(2);
        }
        "tab" => {
            let id = v.as_str().unwrap();
            let area = g.app().prefs.panel(id).area;
            g.app().prefs.active_tab.insert(area, id.to_string());
            g.frames(2);
        }
        "dock" => {
            g.app().prefs.dock_sizes.insert(v[0].as_str().unwrap().to_string(), v[1].as_f64().unwrap() as f32);
            g.frames(2);
        }
        "cut_settings" => {
            g.app().cut_settings_open = Some(v.as_u64().unwrap() as usize);
            g.frames(3);
        }
        "frame" => {
            let r = darkburn_core::kurbo::Rect::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap(), v[2].as_f64().unwrap(), v[3].as_f64().unwrap());
            g.app().view_cmd = Some(crate::canvas::ViewCmd::Frame(r));
            g.frames(3);
        }
        "view" => {
            let mut app = g.app();
            app.prefs.view.zoom = v[0].as_f64().unwrap() as f32;
            app.prefs.view.offset = vec2(v[1].as_f64().unwrap() as f32, v[2].as_f64().unwrap() as f32);
            drop(app);
            g.frames(2);
        }
        "preview" => {
            let mut app = g.app();
            let p = &mut app.job.preview;
            if let Some(x) = v.get("maximised").and_then(Value::as_bool) {
                p.maximised = x;
            }
            if let Some(x) = v.get("travel").and_then(Value::as_bool) {
                p.view.show_travel = x;
            }
            if let Some(x) = v.get("shade").and_then(Value::as_bool) {
                p.view.shade_by_power = x;
            }
            if let Some(x) = v.get("zoom").and_then(Value::as_f64) {
                p.zoom = x as f32;
            }
            if let Some(x) = v.get("pan") {
                p.pan = vec2(x[0].as_f64().unwrap() as f32, x[1].as_f64().unwrap() as f32);
            }
            if let Some(x) = v.get("fitted").and_then(Value::as_bool) {
                p.fitted = x;
            }
            if let Some(x) = v.get("frac").and_then(Value::as_f64) {
                let total = p.data.as_ref().map_or(0.0, |d| d.stats.seconds);
                p.t = total * x;
            }
            println!("preview: zoom {} pan {:?} t {} of {:?}", p.zoom, p.pan, p.t, p.data.as_ref().map(|d| d.stats.seconds));
            drop(app);
            g.frames(3);
        }
        "photo" => {
            let path = g.temp(v["name"].as_str().unwrap());
            photo(v["w"].as_u64().unwrap() as u32, v["h"].as_u64().unwrap() as u32).save(&path).unwrap();
            let ctx = g.ctx();
            let mut args = json!({ "paths": [path.to_string_lossy()] });
            if let Some(x) = v.get("x") {
                args["x"] = x.clone();
                args["y"] = v["y"].clone();
            }
            let r = crate::api::execute(&mut g.app(), &ctx, "project.import", &args);
            g.frames(3);
            println!("photo -> {:?}", r.map(|x| x.to_string()));
        }
        "project_path" => {
            g.app().project_path = Some(std::path::PathBuf::from(v.as_str().unwrap()));
            g.app().editor.mark_saved();
            g.frames(2);
        }
        "park" => {
            g.hover_pos(pos2(-200.0, -200.0));
            g.frames(2);
        }
        "window" => {
            g.set_window_size(v[0].as_f64().unwrap() as f32, v[1].as_f64().unwrap() as f32);
        }
        "allow_clashes" => {
            g.allow_id_clashes();
        }
        "#" => {}
        o => panic!("unknown op {o}"),
    }
}

/// `DARKBURN_SHOT_SCRIPT=<file.json> DARKBURN_SHOT_OUT=<dir> cargo test -p darkburn readme_shots::explore -- --ignored --nocapture`
#[test]
#[ignore = "explorer"]
fn explore() {
    let script = std::env::var("DARKBURN_SHOT_SCRIPT").expect("DARKBURN_SHOT_SCRIPT");
    let out = std::path::PathBuf::from(std::env::var("DARKBURN_SHOT_OUT").expect("DARKBURN_SHOT_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let text = std::fs::read_to_string(&script).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("script: {e}"));
    let size = v.get("size").cloned().unwrap_or(json!([1440.0, 900.0, 2.0]));
    let mut g = Gui::new(size[0].as_f64().unwrap() as f32, size[1].as_f64().unwrap() as f32, size[2].as_f64().unwrap() as f32);
    for op in v["ops"].as_array().unwrap() {
        let t = std::time::Instant::now();
        step(&mut g, op, &out);
        if t.elapsed().as_millis() > 1500 {
            println!("  (slow op {} took {:?})", op, t.elapsed());
        }
    }
}
