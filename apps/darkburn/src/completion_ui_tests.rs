//! Panel and dialog checks that go through a frame of the real UI code (no window): the Variable
//! Text panel [VTP-01 to VTP-05], layer links in a saved project [LAYER-10].

use egui::{RawInput, Rect, pos2, vec2};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;

fn app() -> DarkBurn {
    DarkBurn::new(Prefs::default())
}

fn frame(app: &mut DarkBurn, f: impl FnOnce(&mut egui::Ui, &mut DarkBurn)) {
    let ctx = egui::Context::default();
    let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 900.0))), ..Default::default() };
    let mut f = Some(f);
    let mut out = ctx.run_ui(input, |ui| {
        if let Some(f) = f.take() {
            f(ui, app);
        }
    });
    out.textures_delta.clear();
}

/// The panel draws every field with serial numbers set and a CSV file loaded, and the commands its
/// buttons run change the document's variable state.
#[test]
fn variable_text_panel_draws_and_its_buttons_run_commands() {
    let mut a = app();
    let dir = std::env::temp_dir().join(format!("darkburn-vtp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("rows.csv");
    std::fs::write(&file, "name\nAnn\nBob\nCy\n").unwrap();
    execute_headless(&mut a, "variable.set", &json!({ "start": 10, "end": 20, "current": 12, "advance_by": 2, "padding": 4, "auto_advance": true }))
        .unwrap();
    execute_headless(&mut a, "variable.csv_load", &json!({ "path": file.to_string_lossy(), "header": true })).unwrap();
    frame(&mut a, crate::text_ui::variable_panel);
    let s = &a.editor.doc.variables.serial;
    assert_eq!((s.start, s.end, s.current, s.advance_by, s.padding), (10, 20, 12, 2, 4));
    assert!(a.editor.doc.variables.auto_advance);
    assert_eq!(a.editor.doc.variables.merge.row_count(), 3);
    // Previous, Next, Reset, Bake, Test, CSV clear: the ids the panel's buttons use.
    for id in ["variable.previous", "variable.next", "variable.reset", "variable.bake", "variable.test", "variable.csv_clear"] {
        assert!(crate::commands::lookup(id).is_some_and(|c| crate::commands::works(c.id)), "{id}");
    }
    execute_headless(&mut a, "variable.next", &Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 14);
    execute_headless(&mut a, "variable.previous", &Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 12);
    let t = execute_headless(&mut a, "variable.test", &json!({ "template": "N{serial:4}", "count": 3 })).unwrap();
    assert_eq!(t["values"], json!(["N0012", "N0014", "N0016"]));
    execute_headless(&mut a, "variable.reset", &Value::Null).unwrap();
    assert_eq!(a.editor.doc.variables.serial.current, 10);
    execute_headless(&mut a, "variable.csv_clear", &Value::Null).unwrap();
    frame(&mut a, crate::text_ui::variable_panel);
    assert_eq!(a.editor.doc.variables.merge.row_count(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

/// A layer linked to a library entry keeps the link in a saved project.
#[test]
fn layer_library_links_survive_save_and_load() {
    let mut a = app();
    a.editor.doc.layer_mut(5).library_link = Some("Ply - 3 mm - Cut fast".into());
    let bytes = darkburn_core::project::save(&a.editor.doc, "test", None).unwrap();
    let back = darkburn_core::project::load(&bytes).unwrap();
    assert_eq!(back.doc.layer(5).library_link.as_deref(), Some("Ply - 3 mm - Cut fast"));
    assert!(back.doc.layer(4).library_link.is_none());
}

/// Each device keeps its own panel layout: change it on one, switch, change it, switch back.
#[test]
fn panel_layout_is_remembered_per_device() {
    use crate::devices::{Device, DeviceType};
    let ctx = egui::Context::default();
    let mut a = app();
    let first = a.machine.devices.devices[0].id;
    let second = a.machine.devices.add(Device::new(DeviceType::Grbl));
    a.machine.select_device(first);
    for _ in 0..2 {
        crate::machine_ui::frame(&ctx, &mut a);
    }
    a.prefs.panels.get_mut("console").unwrap().visible = true;
    crate::machine_ui::frame(&ctx, &mut a);
    a.machine.select_device(second);
    crate::machine_ui::frame(&ctx, &mut a);
    a.prefs.panels.get_mut("console").unwrap().visible = false;
    crate::machine_ui::frame(&ctx, &mut a);
    a.machine.select_device(first);
    crate::machine_ui::frame(&ctx, &mut a);
    assert!(a.prefs.panels["console"].visible, "the first device gets its layout back");
    a.machine.select_device(second);
    crate::machine_ui::frame(&ctx, &mut a);
    assert!(!a.prefs.panels["console"].visible, "and the second keeps its own");
}
