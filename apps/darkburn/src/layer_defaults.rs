//! Default layer settings per device [LAYER-07, LAYER-08]: make the project's layers the default
//! of this device (or of every device), put them back on a project, or fall back to the factory
//! layers. A new project starts with the device's defaults when the setting asks for it.

use darkburn_core::layer::{self, Layer};
use serde_json::{Value, json};

use crate::commands::Cmd;
use crate::shell::DarkBurn;

const fn cmd(id: &'static str, label: &'static str, args: &'static str) -> Cmd {
    Cmd { id, label, key: "", icon: "", args }
}

pub const COMMANDS: &[Cmd] = &[
    cmd(
        "layer.default_make",
        "Make the Layers the Default",
        r#"{"type":"object","properties":{"all_devices":{"type":"boolean","description":"also make them the default of every other device"}}}"#,
    ),
    cmd("layer.default_apply", "Load the Device's Default Layers", r#"{"type":"object","properties":{}}"#),
    cmd(
        "layer.default_reset",
        "Reset the Default Layers",
        r#"{"type":"object","properties":{"all_devices":{"type":"boolean"},"project":{"type":"boolean","description":"also put the factory layers on the open project"}}}"#,
    ),
];

fn parse(text: &str) -> Option<Vec<Layer>> {
    let layers = serde_json::from_str::<Vec<Layer>>(text).ok()?;
    (layers.len() == layer::LAYER_COUNT).then_some(layers)
}

/// The open project's layers become the active device's defaults (and every device's with
/// `all`). Returns how many devices were changed.
pub fn make_default(app: &mut DarkBurn, all: bool) -> usize {
    let Ok(json) = serde_json::to_string(&app.editor.doc.layers) else { return 0 };
    let active = app.machine.devices.active;
    let mut n = 0;
    for d in &mut app.machine.devices.devices {
        if all || Some(d.id) == active {
            d.state.default_layers = Some(json.clone());
            n += 1;
        }
    }
    n
}

/// The device's default layers, if it has any.
pub fn device_layers(app: &DarkBurn) -> Option<Vec<Layer>> {
    app.machine.active().state.default_layers.as_deref().and_then(parse)
}

/// Put the device's defaults on the open project as one undo step.
pub fn apply(app: &mut DarkBurn) -> bool {
    let Some(layers) = device_layers(app) else { return false };
    app.editor.edit("Load default layers", |d, _| {
        d.layers = layers;
        true
    })
}

/// Forget the defaults (this device's, or all), and optionally put the factory layers on the project.
pub fn reset(app: &mut DarkBurn, all: bool, project: bool) -> usize {
    let active = app.machine.devices.active;
    let mut n = 0;
    for d in &mut app.machine.devices.devices {
        if (all || Some(d.id) == active) && d.state.default_layers.take().is_some() {
            n += 1;
        }
    }
    if project {
        app.editor.edit("Reset layers", |d, _| {
            d.layers = layer::default_layers();
            true
        });
    }
    n
}

/// A fresh project: the device's default layers when the setting is on (no undo step; the
/// project is not modified by them).
pub fn on_new_project(app: &mut DarkBurn) {
    if !app.prefs.opts.default_layers_on_new {
        return;
    }
    if let Some(layers) = device_layers(app) {
        app.editor.doc.layers = layers;
    }
}

pub fn execute(app: &mut DarkBurn, id: &str, a: &Value) -> Option<Result<Value, String>> {
    let flag = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(match id {
        "layer.default_make" => {
            let n = make_default(app, flag("all_devices"));
            if n == 0 { Err("there is no device to store the layers for".into()) } else { Ok(json!({ "devices": n })) }
        }
        "layer.default_apply" => {
            if apply(app) {
                Ok(json!({ "applied": true }))
            } else {
                Err("this device has no default layers".into())
            }
        }
        "layer.default_reset" => Ok(json!({ "devices": reset(app, flag("all_devices"), flag("project")) })),
        _ => return None,
    })
}

/// A menu or button click (no arguments).
pub fn run(app: &mut DarkBurn, _ctx: &egui::Context, id: &str) -> bool {
    match execute(app, id, &Value::Null) {
        Some(Err(e)) => {
            app.note = Some(e);
            true
        }
        Some(Ok(_)) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::execute_headless;
    use crate::devices::{Device, DeviceType};
    use crate::prefs::Prefs;

    fn app() -> DarkBurn {
        DarkBurn::new(Prefs::default())
    }

    #[test]
    fn make_apply_reset_on_the_device() {
        let mut a = app();
        a.editor.doc.layers[3].name = "Mine".into();
        a.editor.doc.layers[3].main_mut().speed = 77.0;
        execute_headless(&mut a, "layer.default_make", &json!({})).unwrap();
        a.editor.doc.layers[3].main_mut().speed = 5.0;
        execute_headless(&mut a, "layer.default_apply", &json!({})).unwrap();
        assert_eq!(a.editor.doc.layers[3].main().speed, 77.0);
        assert_eq!(a.editor.doc.layers[3].name, "Mine");
        // Apply is one undo step.
        a.editor.undo();
        assert_eq!(a.editor.doc.layers[3].main().speed, 5.0);
        // Reset forgets the default and (asked to) the project's layers.
        execute_headless(&mut a, "layer.default_reset", &json!({ "project": true })).unwrap();
        assert!(execute_headless(&mut a, "layer.default_apply", &json!({})).is_err());
        assert_eq!(a.editor.doc.layers[3].main().speed, layer::default_layers()[3].main().speed);
    }

    #[test]
    fn make_default_for_all_devices() {
        let mut a = app();
        a.machine.devices.add(Device::new(DeviceType::Ruida));
        a.editor.doc.layers[1].main_mut().passes = 4;
        let r = execute_headless(&mut a, "layer.default_make", &json!({ "all_devices": true })).unwrap();
        assert_eq!(r["devices"].as_u64().unwrap() as usize, a.machine.devices.devices.len());
        assert!(a.machine.devices.devices.iter().all(|d| d.state.default_layers.is_some()));
        // Making it for the active device only leaves the others as they were.
        reset(&mut a, true, false);
        execute_headless(&mut a, "layer.default_make", &json!({})).unwrap();
        assert_eq!(a.machine.devices.devices.iter().filter(|d| d.state.default_layers.is_some()).count(), 1);
    }

    #[test]
    fn a_new_project_starts_with_the_defaults_when_the_setting_is_on() {
        let mut a = app();
        a.editor.doc.layers[5].main_mut().max_power = 33.0;
        make_default(&mut a, false);
        a.prefs.opts.default_layers_on_new = true;
        crate::files::new_project(&mut a);
        assert_eq!(a.editor.doc.layers[5].main().max_power, 33.0);
        assert!(!a.editor.is_modified(), "a blank project with default layers is not unsaved work");
        a.prefs.opts.default_layers_on_new = false;
        crate::files::new_project(&mut a);
        assert_eq!(a.editor.doc.layers[5].main().max_power, layer::default_layers()[5].main().max_power);
    }

    #[test]
    fn the_commands_are_registered_and_work() {
        for c in COMMANDS {
            assert!(crate::commands::lookup(c.id).is_some(), "{}", c.id);
            assert!(crate::commands::works(c.id));
        }
    }
}
