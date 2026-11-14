//! The quick recipe picker [MCC-04]: search ranking, putting a recipe on a layer, the recent list
//! and taking a recipe back. The package is a made-up one written here and is only read.

use std::path::Path;

use darkburn_mcc::recipe::{co2_from_entry, co2_template, fibre_from_entry, fibre_template};
use darkburn_mcc::{Craft, GasType, write_recipe};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::gweike_workflow::Head;
use crate::prefs::Prefs;
use crate::recipe_picker::{RECENTS, materials, picks, picks_of};
use crate::shell::DarkBurn;

const MILD: &str = "Cutting parameters/1200W/Mild steel/Mild 2.0mm 1.5S F-2 O2.xml";
const ACR24: &str = "Cutting parameters/CO2/Acrylic/Acrylic 24mm air.xml";
const ACR3: &str = "Cutting parameters/CO2/Acrylic/Acrylic 3mm air.xml";
const PLY3: &str = "Cutting parameters/CO2/Plywood/Plywood 3mm air.xml";
const BAD: &str = "Cutting parameters/1200W/Steel/Steel 1mm 1.5S F0 N2.xml";

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn package(root: &Path) {
    let mut f = fibre_from_entry(&fibre_template()).0;
    f.name = "Mild 2.0mm 1.5S F-2 O2".into();
    f.cut.gas = GasType::O2;
    f.cut.speed_mm_s = 33.0;
    put(root, MILD, &write_recipe(&Craft::Fibre(f)).unwrap());
    put(
        root,
        BAD,
        b"<ParameterRoot>\r\n<PLayerParam11>\r\n<GP CutSpeed=\"20\" CutPower=\"150\" CutFreq=\"5000\" CutGasType=\"2\" CutAirPressure=\"0.8\" ManuType=\"0\" LayerFileName=\"Steel 1mm 1.5S F0 N2\"/>\r\n</PLayerParam11>\r\n</ParameterRoot>\r\n",
    );
    for (rel, name, speed) in [(ACR24, "Acrylic 24mm air", 2.0), (ACR3, "Acrylic 3mm air", 15.0), (PLY3, "Plywood 3mm air", 20.0)] {
        let mut c = co2_from_entry(&co2_template()).0;
        c.name = name.into();
        c.speed_mm_s = speed;
        put(root, rel, &write_recipe(&Craft::Co2(c)).unwrap());
    }
}

fn app_with_package(tag: &str) -> (DarkBurn, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("darkburn_pick_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    package(&dir);
    let mut a = DarkBurn::new(Prefs::default());
    let id = run(&mut a, "device.add", json!({ "type": "GweikeCardSimulator" })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    run(&mut a, "gweike.recipes_scan", json!({ "path": dir.display().to_string() })).unwrap();
    (a, dir)
}

#[test]
fn search_finds_material_and_thickness_and_ranks_the_exact_thickness_first() {
    let (a, dir) = app_with_package("rank");
    let lib = a.machine.ext.gweike_recipes.library.as_ref().unwrap();
    let all = picks(lib, "", &[]);
    assert_eq!(all.len(), 5);
    let p = &all.iter().find(|p| p.row.key == ACR24).unwrap();
    assert_eq!(p.label, "Acrylic · 24 mm · CO2 · air");
    assert!(p.values.starts_with("2 mm/s (0.12 m/min)"), "{}", p.values);

    let acr = picks(lib, "acr 24", &[]);
    assert_eq!(acr.iter().map(|p| p.row.key.as_str()).collect::<Vec<_>>(), [ACR24]);
    let three = picks(lib, "3", &[]);
    assert_eq!(three.first().map(|p| p.row.key.as_str()), Some(ACR3), "the exact thickness goes first");
    assert!(three.iter().any(|p| p.row.key == PLY3));
    assert_eq!(picks(lib, "ply 3", &[]).len(), 1);
    assert_eq!(picks(lib, "MILD 2", &[])[0].row.key, MILD);
    assert!(picks(lib, "walnut", &[]).is_empty());
    // A recipe the checks refuse is listed with its errors, so the picker shows it greyed out.
    assert!(!picks(lib, "steel 1", &[])[0].row.errors.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_recipe_goes_on_a_layer_with_its_own_slot_and_head() {
    let (mut a, dir) = app_with_package("apply");
    run(&mut a, "gweike.layer_map", json!({ "layer": 1, "head": "co2", "slot": 1 })).unwrap();
    let r = run(&mut a, "gweike.recipe_layer", json!({ "recipe": ACR24, "layer": 0 })).unwrap();
    assert_eq!((r["head"].as_str(), r["layer"].as_u64()), (Some("co2"), Some(0)), "{r}");
    let s = a.machine.active().settings.gweike.clone();
    let m = s.layers.get(&0).unwrap();
    assert_eq!((m.head, m.material.as_str()), (Head::Co2, "Acrylic"));
    assert_eq!(s.crafts.co2[m.slot as usize].speed_mm_s, 2.0);
    assert!(r["message"].as_str().unwrap().contains("Acrylic · 24 mm"));
    assert_eq!(a.machine.ext.gweike_recipes.pick_message.as_deref(), r["message"].as_str());

    // The current layer is the default; a refused recipe changes nothing.
    a.editor.current_layer = 2;
    let before = a.machine.active().settings.gweike.clone();
    assert!(run(&mut a, "gweike.recipe_layer", json!({ "recipe": BAD })).is_err());
    assert_eq!(a.machine.active().settings.gweike, before);
    run(&mut a, "gweike.recipe_layer", json!({ "recipe": "Plywood 3mm air" })).unwrap();
    assert_eq!(a.machine.active().settings.gweike.layers.get(&2).unwrap().material, "Plywood");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_layers_with_recipes_of_one_head_get_different_slots() {
    let (mut a, dir) = app_with_package("slots");
    run(&mut a, "shape.add_rect", json!({ "x": 10, "y": 10, "width": 20, "height": 20, "layer": 0 })).unwrap();
    run(&mut a, "shape.add_rect", json!({ "x": 50, "y": 10, "width": 20, "height": 20, "layer": 11 })).unwrap();
    // Layer 11 would default to slot 0, the same as layer 0: the second recipe moves on.
    let a0 = run(&mut a, "gweike.recipe_layer", json!({ "recipe": ACR3, "layer": 0 })).unwrap();
    let a1 = run(&mut a, "gweike.recipe_layer", json!({ "recipe": PLY3, "layer": 11 })).unwrap();
    assert_ne!(a0["slot"], a1["slot"]);
    let s = a.machine.active().settings.gweike.clone();
    assert_eq!(s.crafts.co2[a0["slot"].as_u64().unwrap() as usize].speed_mm_s, 15.0);
    assert_eq!(s.crafts.co2[a1["slot"].as_u64().unwrap() as usize].speed_mm_s, 20.0);
    // Putting a new recipe on a layer keeps its own slot.
    let again = run(&mut a, "gweike.recipe_layer", json!({ "recipe": ACR24, "layer": 0 })).unwrap();
    assert_eq!(again["slot"], a0["slot"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recent_recipes_come_first_and_the_list_is_short() {
    let (mut a, dir) = app_with_package("recent");
    run(&mut a, "gweike.recipe_layer", json!({ "recipe": PLY3, "layer": 0 })).unwrap();
    run(&mut a, "gweike.recipe_layer", json!({ "recipe": MILD, "layer": 0 })).unwrap();
    let recents = a.machine.active().settings.gweike.recipe_recents.clone();
    assert_eq!(recents, [MILD, PLY3]);
    let lib = a.machine.ext.gweike_recipes.library.as_ref().unwrap();
    let list = picks(lib, "", &recents);
    assert_eq!((list[0].row.key.as_str(), list[0].recent), (MILD, true));
    assert_eq!((list[1].row.key.as_str(), list[1].recent), (PLY3, true));
    assert!(!list[2].recent);
    // Search still filters the recent ones.
    let acr = picks(lib, "acr", &recents);
    assert_eq!(acr.len(), 2);
    assert!(acr.iter().all(|p| p.label.starts_with("Acrylic") && !p.recent));
    for _ in 0..3 {
        for k in [ACR3, ACR24, PLY3] {
            run(&mut a, "gweike.recipe_layer", json!({ "recipe": k, "layer": 0 })).unwrap();
        }
    }
    let recents = a.machine.active().settings.gweike.recipe_recents.clone();
    assert!(recents.len() <= RECENTS);
    assert_eq!(recents[0], PLY3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_takes_back_the_recipe_and_the_folder_is_remembered() {
    let (mut a, dir) = app_with_package("undo");
    let before = a.machine.active().settings.gweike.clone();
    assert_eq!(before.recipe_folder, dir.display().to_string());
    run(&mut a, "gweike.recipe_layer", json!({ "recipe": ACR24, "layer": 3 })).unwrap();
    assert_ne!(a.machine.active().settings.gweike, before);
    run(&mut a, "gweike.recipe_undo", json!({})).unwrap();
    assert_eq!(a.machine.active().settings.gweike, before);
    assert!(run(&mut a, "gweike.recipe_undo", json!({})).is_err(), "nothing left to take back");

    // A new session reads the remembered folder when the picker opens.
    a.machine.ext.gweike_recipes = Default::default();
    crate::recipe_picker::open(&mut a, 3);
    assert_eq!(a.machine.ext.gweike_recipes.picker, Some(3));
    assert!(a.machine.ext.gweike_recipes.library.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_material_filter_keeps_one_material() {
    let (a, dir) = app_with_package("material");
    let lib = a.machine.ext.gweike_recipes.library.as_ref().unwrap();
    let mats = materials(lib);
    assert!(mats.iter().any(|m| m == "Acrylic") && mats.iter().any(|m| m == "Plywood"), "{mats:?}");
    let acr = picks_of(lib, "", "acrylic", &[]);
    assert_eq!(acr.len(), 2);
    assert!(acr.iter().all(|p| p.material == "Acrylic"));
    assert_eq!(picks_of(lib, "3", "Acrylic", &[]).iter().map(|p| p.row.key.as_str()).collect::<Vec<_>>(), [ACR3]);
    assert_eq!(picks_of(lib, "", "", &[]).len(), picks(lib, "", &[]).len());
    let _ = std::fs::remove_dir_all(&dir);
}
