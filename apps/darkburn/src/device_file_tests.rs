//! Device files keep everything: every field of every device type, set to something other than
//! what a new device has, comes back from the device list file, from a `.dbdev` export and
//! import, and from a copy of the device.
//!
//! The fields are found by walking the device as JSON, so a field added later is covered without
//! touching this file. Hardware safety: nothing here connects to anything; only files in a
//! temporary folder are written.

use serde_json::Value;

use crate::devices::{Device, DeviceStore, DeviceType};

const TYPES: [DeviceType; 29] = [
    DeviceType::Grbl,
    DeviceType::GrblM3,
    DeviceType::GrblLpc,
    DeviceType::GrblHal,
    DeviceType::FluidNc,
    DeviceType::Ruida,
    DeviceType::SimulatorGrbl,
    DeviceType::SimulatorRuida,
    DeviceType::DesignOnly,
    DeviceType::Smoothie,
    DeviceType::Marlin,
    DeviceType::Snapmaker,
    DeviceType::LinuxCnc,
    DeviceType::Emblaser1,
    DeviceType::Emblaser2,
    DeviceType::CustomGcode,
    DeviceType::SimulatorSmoothie,
    DeviceType::SimulatorMarlin,
    DeviceType::SimulatorSnapmaker,
    DeviceType::SimulatorCustom,
    DeviceType::Galvo,
    DeviceType::SimulatorGalvo,
    DeviceType::GweikeCardSimulator,
    DeviceType::SnapmakerNetwork,
    DeviceType::EmblaserWifi,
    DeviceType::Cubiio,
    DeviceType::Trocen,
    DeviceType::TopWisdom,
    DeviceType::Grbl,
];

/// Every leaf of `v` as a path of keys and indexes.
pub(crate) fn leaves(v: &Value, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match v {
        Value::Object(m) if !m.is_empty() => {
            for (k, x) in m {
                at.push(k.clone());
                leaves(x, at, out);
                at.pop();
            }
        }
        Value::Array(a) if !a.is_empty() => {
            for (i, x) in a.iter().enumerate() {
                at.push(i.to_string());
                leaves(x, at, out);
                at.pop();
            }
        }
        _ => out.push(at.clone()),
    }
}

pub(crate) fn at<'a>(v: &'a Value, path: &[String]) -> &'a Value {
    path.iter().fold(v, |v, k| match v {
        Value::Array(a) => k.parse::<usize>().ok().and_then(|i| a.get(i)).unwrap_or(&Value::Null),
        _ => v.get(k).unwrap_or(&Value::Null),
    })
}

pub(crate) fn at_mut<'a>(v: &'a mut Value, path: &[String]) -> &'a mut Value {
    path.iter().fold(v, |v, k| match v {
        Value::Array(a) => &mut a[k.parse::<usize>().unwrap()],
        _ => v.get_mut(k).unwrap(),
    })
}

/// Other values a leaf could hold. Several are offered because not every one is a value the
/// field takes (an enum written as text, a whole number, a number with a range).
pub(crate) fn others(v: &Value) -> Vec<Value> {
    match v {
        Value::Bool(b) => vec![Value::Bool(!b)],
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            let mut out = vec![serde_json::json!(f + 1.0), serde_json::json!(f + 0.25), serde_json::json!(f * 0.5 + 3.0)];
            if let Some(i) = n.as_i64() {
                out.insert(0, serde_json::json!(i + 1));
                out.push(serde_json::json!((i - 1).max(0)));
            }
            out
        }
        Value::String(s) => vec![Value::String(format!("{s}x")), Value::String("10.0.0.7".into())],
        Value::Null => vec![serde_json::json!("text"), serde_json::json!(2.5), serde_json::json!([1.5, 2.5]), serde_json::json!(true)],
        Value::Array(_) => vec![serde_json::json!(["one"]), serde_json::json!([1.5])],
        Value::Object(_) => Vec::new(),
    }
}

fn through_the_list_file(d: &Device, tag: &str) -> Device {
    let dir = std::env::temp_dir().join(format!("darkburn-devfile-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut store = DeviceStore::default();
    let id = store.add(d.clone());
    store.save(&dir).unwrap();
    let back = DeviceStore::load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(back, store, "the device list file");
    back.get(id).unwrap().clone()
}

/// What a file may change: the id and the name are given by the list a device is added to.
fn comparable(d: &Device) -> Value {
    let mut v = serde_json::to_value(d).unwrap();
    v["id"] = Value::Null;
    v["name"] = Value::Null;
    v
}

/// The lists and tables a new device has empty, filled in.
fn with_the_lists_filled(mut d: Device) -> Device {
    d.settings.scan.offsets.entries =
        vec![crate::scan_offsets::OffsetEntry { speed: 100.0, offset: 0.05 }, crate::scan_offsets::OffsetEntry { speed: 300.0, offset: 0.15 }];
    d.state.saved_positions = vec![crate::machine_ui::SavedPos { name: "Corner".into(), x: 12.5, y: 30.25, z: 4.0, use_z: true }];
    d.state.finish_position = Some((101.5, 22.25));
    d.state.panel_layout = Some("{\"laser\":{\"area\":\"Left\",\"visible\":true}}".into());
    d.state.default_layers = Some("[]".into());
    d.state.excluded_ports = vec!["COM7".into(), "COM9".into()];
    d.state.macros = ["G0 X0".into(), "M8".into(), "M9".into(), "$H".into(), "G0 Y5".into(), "?".into()];
    d.state.port = "COM4".into();
    d.settings.checklist = vec!["Lens is clean".into()];
    d.settings.gweike.layers.insert(3, Default::default());
    d
}

#[test]
fn every_field_of_every_device_type_survives_the_device_files() {
    let defaults: Vec<Value> = TYPES.iter().map(|k| serde_json::to_value(Device::new(*k)).unwrap()).collect();
    let mut tried = 0;
    let mut lost: Vec<String> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for (n, kind) in TYPES.into_iter().enumerate() {
        let base = with_the_lists_filled(Device::new(kind));
        let json = serde_json::to_value(&base).unwrap();
        let mut paths = Vec::new();
        leaves(&json, &mut Vec::new(), &mut paths);
        assert!(paths.len() > 100, "{kind:?}: only {} fields found", paths.len());

        // The device with its lists filled, through the file on disk and as a copy.
        let back = through_the_list_file(&base, "list");
        assert_eq!(comparable(&back), comparable(&base), "{kind:?}: the device list file");
        let mut store = DeviceStore::default();
        let id = store.add(base.clone());
        let copy = store.duplicate(id).and_then(|c| store.get(c).cloned()).unwrap();
        assert_eq!(comparable(&copy), comparable(&base), "{kind:?}: a copy of the device");

        for path in &paths {
            let name = path.join(".");
            // The craft table is the same in every device: it is walked for the first type and
            // for the Gweike one.
            let crafts = name.starts_with("settings.gweike.crafts");
            if matches!(name.as_str(), "id" | "name" | "kind") || (crafts && n > 0 && kind != DeviceType::GweikeCardSimulator) {
                continue;
            }
            // The first other value the field takes: a changed one, or what another device type has there.
            let old = at(&json, path);
            let mut candidates = others(old);
            candidates.extend(defaults.iter().map(|d| at(d, path).clone()).filter(|v| v != old && !v.is_null()));
            let changed = candidates.into_iter().find_map(|new| {
                let mut j = json.clone();
                *at_mut(&mut j, path) = new.clone();
                let d: Device = serde_json::from_value(j).ok()?;
                (at(&serde_json::to_value(&d).unwrap(), path) == &new).then_some((d, new))
            });
            let Some((d, new)) = changed else {
                refused.push(name);
                continue;
            };
            tried += 1;
            let what = format!("{kind:?} {name} = {new}");
            // The text of the device list file.
            let mut store = DeviceStore::default();
            let id = store.add(d.clone());
            let back = DeviceStore::from_json(&store.to_json());
            if back != store {
                lost.push(format!("list file: {what}"));
            }
            // Export and import (.dbdev). A device the app would not accept is not a file anyone has.
            if !d.validate().is_empty() {
                continue;
            }
            let text = store.export_profile(id).unwrap();
            match store.import_profile(&text) {
                Ok(new_id) => {
                    if comparable(store.get(new_id).unwrap()) != comparable(&d) {
                        lost.push(format!("export and import: {what}"));
                    }
                }
                Err(e) => lost.push(format!("export and import: {what}: refused: {e}")),
            }
        }
    }
    assert!(lost.is_empty(), "{} of {tried} fields were lost:\n{}", lost.len(), lost.join("\n"));
    assert!(tried > 3000, "only {tried} fields were tried");
    // Fields no other value was found for (choices no device type has differently). Listed here
    // so that a new one shows up.
    let digits = |s: &str| s.split('.').map(|p| if p.parse::<usize>().is_ok() { "N" } else { p }).collect::<Vec<_>>().join(".");
    let mut names: Vec<String> = refused.iter().map(|r| digits(r)).collect();
    names.sort();
    names.dedup();
    let known = [
        "settings.conn.sim_firmware",
        "settings.galvo.ports.done_bit",
        "settings.galvo.ports.door_bit",
        "settings.galvo.source",
        "settings.gcode.air",
        "settings.gcode.marlin_power",
        "settings.gcode.tool_index",
        "settings.gweike.crafts.co2.N.freq_curve",
        "settings.gweike.crafts.co2.N.gas",
        "settings.gweike.crafts.co2.N.power_curve",
        "settings.gweike.crafts.co2.N.slow_start",
        "settings.gweike.crafts.fibre.N.cut.gas",
        "settings.gweike.crafts.fibre.N.freq_curve",
        "settings.gweike.crafts.fibre.N.pierce",
        "settings.gweike.crafts.fibre.N.power_curve",
        "settings.gweike.crafts.fibre.N.process",
        "settings.gweike.crafts.fibre.N.slow_start",
        "settings.gweike.crafts.fibre.N.smooth_pierce",
        "settings.gweike.layers.N.head",
        "state.backups",
    ];
    assert_eq!(names, known, "no other value was found for these");
}

/// The same for the preferences file: every setting that is exported comes back on import,
/// except the two that belong to this computer (the recent projects and the AI agent settings).
#[test]
fn every_preference_survives_export_and_import() {
    use crate::prefs::Prefs;
    let dir = std::env::temp_dir().join(format!("darkburn-prefs-fields-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("prefs.json");
    let json: Value = serde_json::from_str(&Prefs::default().to_json()).unwrap();
    let mut paths = Vec::new();
    leaves(&json, &mut Vec::new(), &mut paths);
    assert!(paths.len() > 120, "only {} settings found", paths.len());
    let (mut tried, mut lost) = (0, Vec::new());
    for path in &paths {
        let name = path.join(".");
        // The pendant, like agent access, is only switched on at this computer.
        if path[0] == "recent" || path[0] == "mcp" || name == "opts.pendant" {
            continue;
        }
        // The first other value the setting keeps (settings are brought into range when read).
        let changed = others(at(&json, path)).into_iter().find_map(|new| {
            let mut j = json.clone();
            *at_mut(&mut j, path) = new.clone();
            let p = Prefs::from_json(&j.to_string());
            let back: Value = serde_json::from_str(&p.to_json()).unwrap();
            (at(&back, path) == &new).then_some((p, new))
        });
        let Some((theirs, new)) = changed else { continue };
        tried += 1;
        crate::prefs_files::export(&theirs, &file).unwrap();
        let mut mine = Prefs::default();
        crate::prefs_files::import(&mut mine, &file, None).unwrap();
        if mine != theirs {
            lost.push(format!("{name} = {new}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(lost.is_empty(), "{} of {tried} settings were lost:\n{}", lost.len(), lost.join("\n"));
    assert!(tried > 90, "only {tried} settings were tried");
}
