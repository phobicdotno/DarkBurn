//! Fast recipe switching for the Gweike M3 Ultra [MCC-04]: a settings package folder is read, its
//! recipes are listed per head and one goes on a craft slot with one command or one click. The
//! package is a made-up one written here; nothing reaches a machine and the package is only read.

use std::path::Path;

use darkburn_mcc::recipe::{co2_from_entry, co2_template, fibre_from_entry, fibre_template};
use darkburn_mcc::{Craft, GasType, write_recipe};
use serde_json::{Value, json};

use crate::api::execute_headless;
use crate::gweike_workflow::Head;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use crate::ui_harness::Gui;

const MILD: &str = "Cutting parameters/1200W/Mild steel/Mild 2.0mm 1.5S F-2 O2.xml";
const STEEL: &str = "Cutting parameters/1200W/Steel/Steel 1mm 1.5S F0 N2.xml";
const BIRCH: &str = "Cutting parameters/CO2/Birch/Birch 4mm air.xml";
const BIRCH2: &str = "Cutting parameters/CO2/Birch thin/Birch 4mm air.xml";

fn run(app: &mut DarkBurn, id: &str, args: Value) -> Result<Value, String> {
    execute_headless(app, id, &args)
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A small package: two fibre recipes (one with a duty of 150 %, written by hand as the library
/// refuses to write it) and two CO2 recipes that share a name in different folders.
fn package(root: &Path) {
    let mut f = fibre_from_entry(&fibre_template()).0;
    f.name = "Mild 2.0mm 1.5S F-2 O2".into();
    f.cut.gas = GasType::O2;
    f.cut.gas_pressure_bar = 0.8;
    f.cut.focus_mm = -2.0;
    f.cut.speed_mm_s = 33.0;
    put(root, MILD, &write_recipe(&Craft::Fibre(f)).unwrap());
    put(
        root,
        STEEL,
        b"<ParameterRoot>\r\n<PLayerParam11>\r\n<GP CutSpeed=\"20\" CutPower=\"150\" CutFreq=\"5000\" CutGasType=\"2\" CutAirPressure=\"0.8\" ManuType=\"0\" LayerFileName=\"Steel 1mm 1.5S F0 N2\"/>\r\n</PLayerParam11>\r\n</ParameterRoot>\r\n",
    );
    let mut c = co2_from_entry(&co2_template()).0;
    c.name = "Birch 4mm air".into();
    c.speed_mm_s = 17.0;
    put(root, BIRCH, &write_recipe(&Craft::Co2(c.clone())).unwrap());
    c.speed_mm_s = 25.0;
    put(root, BIRCH2, &write_recipe(&Craft::Co2(c)).unwrap());
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    [MILD, STEEL, BIRCH, BIRCH2].iter().map(|r| (r.to_string(), std::fs::read(root.join(r)).unwrap())).collect()
}

fn gweike_app() -> DarkBurn {
    let mut a = DarkBurn::new(Prefs::default());
    let id = run(&mut a, "device.add", json!({ "type": "GweikeCardSimulator" })).unwrap()["id"].as_u64().unwrap();
    run(&mut a, "device.select", json!({ "id": id })).unwrap();
    a
}

#[test]
fn recipes_are_read_listed_and_put_on_a_craft_slot() {
    let dir = std::env::temp_dir().join(format!("darkburn_gw_recipes_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    package(&dir);
    let before = snapshot(&dir);
    let mut a = gweike_app();

    // Nothing to list or use before a package is read; a file is not a package folder.
    assert!(run(&mut a, "gweike.recipes", json!({})).is_err());
    assert!(run(&mut a, "gweike.recipe_use", json!({ "recipe": "Birch 4mm air" })).is_err());
    assert!(run(&mut a, "gweike.recipes_scan", json!({ "path": dir.join(MILD).display().to_string() })).is_err());

    let r = run(&mut a, "gweike.recipes_scan", json!({ "path": dir.display().to_string() })).unwrap();
    assert_eq!((r["recipes"].as_u64(), r["fibre"].as_u64(), r["co2"].as_u64(), r["refused"].as_u64()), (Some(4), Some(2), Some(2), Some(1)), "{r}");

    // Per head, and by words of the name, path or material.
    let fibre = run(&mut a, "gweike.recipes", json!({ "head": "fibre" })).unwrap()["recipes"].as_array().unwrap().clone();
    assert_eq!(fibre.len(), 2, "{fibre:?}");
    assert!(fibre.iter().all(|r| r["head"] == "fibre"));
    let mild = run(&mut a, "gweike.recipes", json!({ "filter": "MILD o2" })).unwrap()["recipes"].as_array().unwrap().clone();
    assert_eq!(mild.len(), 1, "{mild:?}");
    assert_eq!(mild[0]["recipe"], MILD);
    assert_eq!(mild[0]["usable"], true);
    assert!(mild[0]["detail"].as_str().unwrap().contains("Mild steel"), "{mild:?}");
    let steel = fibre.iter().find(|r| r["recipe"] == STEEL).unwrap();
    assert_eq!(steel["usable"], false);
    assert!(steel["errors"].as_array().unwrap().iter().any(|e| e.as_str().unwrap().starts_with("cut.power_pct")), "{steel}");
    assert!(run(&mut a, "gweike.recipes", json!({ "head": "laser" })).is_err());

    // One command puts a recipe on a slot and opens it in the craft editor.
    let u = run(&mut a, "gweike.recipe_use", json!({ "recipe": "mild 2.0mm 1.5s f-2 o2", "slot": 4 })).unwrap();
    assert_eq!((u["head"].as_str(), u["slot"].as_u64()), (Some("fibre"), Some(4)), "{u}");
    let s = a.machine.active().settings.gweike.clone();
    assert_eq!(s.crafts.fibre[4].cut.speed_mm_s, 33.0);
    assert_eq!(s.crafts.fibre[4].name, "Mild 2.0mm 1.5S F-2 O2");
    assert_eq!((a.machine.ext.gweike_craft_head, a.machine.ext.gweike_craft_slot), (Head::Fibre, 4));

    // Two recipes share a name: the path picks one (either slash), the name alone is refused.
    let e = run(&mut a, "gweike.recipe_use", json!({ "recipe": "Birch 4mm air" })).unwrap_err();
    assert!(e.contains("More than one"), "{e}");
    run(&mut a, "gweike.recipe_use", json!({ "recipe": BIRCH2.replace('/', "\\") })).unwrap();
    let s = a.machine.active().settings.gweike.clone();
    assert_eq!(s.crafts.co2[4].speed_mm_s, 25.0, "no slot given: the one open in the editor");
    assert_eq!(a.machine.ext.gweike_craft_head, Head::Co2);

    // A recipe with a value the machine does not allow is not used; the slot keeps its craft.
    let kept = s.crafts.fibre[5].clone();
    let e = run(&mut a, "gweike.recipe_use", json!({ "recipe": STEEL, "slot": 5 })).unwrap_err();
    assert!(e.contains("power"), "{e}");
    assert_eq!(a.machine.active().settings.gweike.crafts.fibre[5], kept);
    assert!(run(&mut a, "gweike.recipe_use", json!({ "recipe": MILD, "slot": 11 })).is_err());
    assert!(run(&mut a, "gweike.recipe_use", json!({ "recipe": "No such recipe" })).is_err());

    // The craft goes out with the device's craft table, the package is only read.
    let c = run(&mut a, "gweike.crafts", Value::Null).unwrap();
    assert_eq!(c["crafts"]["fibre"][4]["name"], "Mild 2.0mm 1.5S F-2 O2");
    assert_eq!(snapshot(&dir), before, "the package is never written");
    for id in ["gweike.recipes_scan", "gweike.recipes", "gweike.recipe_use"] {
        assert!(crate::machine_ext::gweike(id) && !crate::machine_ext::writes_files(id), "{id}");
        assert!(!crate::commands::describe(id).unwrap().hardware, "{id}");
        assert_eq!(crate::machine_ext::checklist(id), ["MCC-04"]);
    }
    assert!(!a.machine.handle.is_connected());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_gweike_window_switches_recipes_with_one_click() {
    let mut g = Gui::new(1400.0, 1400.0, 1.0);
    {
        let mut a = g.app();
        let id = run(&mut a, "device.add", json!({ "type": "GweikeCardSimulator" })).unwrap()["id"].as_u64().unwrap();
        run(&mut a, "device.select", json!({ "id": id })).unwrap();
        run(&mut a, "shape.add_rect", json!({ "x": 100, "y": 100, "width": 200, "height": 120, "layer": 0 })).unwrap();
        run(&mut a, "gweike.open", Value::Null).unwrap();
    }
    g.frames(4);
    assert!(g.has_text("Recipes from the Gweike software"), "{:?}", g.visible_texts());
    let pkg = g.temp("package");
    package(&pkg);
    g.answer_dialog(&pkg);
    g.click_label("Open package folder...");
    g.frames(4);
    assert!(g.dialog_log().iter().any(|l| l.contains("pick_folder")), "{:?}", g.dialog_log());
    assert!(g.has_text("4 recipes found, 1 with a value the machine does not allow."), "{:?}", g.visible_texts());
    // The fibre head is open in the editor, so the fibre recipes are listed.
    assert_eq!(g.find_texts("Mild 2.0mm 1.5S F-2 O2").len(), 1, "{:?}", g.visible_texts());
    assert!(g.find_texts("Birch 4mm air").is_empty());

    let use_on = |g: &Gui, title: &str| {
        let t = g.find_texts(title)[0];
        g.find_texts("Use").into_iter().find(|r| (r.center().y - t.center().y).abs() < 10.0).unwrap_or_else(|| panic!("no Use button by {title}"))
    };
    // The flagged recipe's button does nothing.
    let before = g.app().machine.active().settings.gweike.crafts.fibre[0].clone();
    let p = use_on(&g, "Steel 1mm 1.5S F0 N2").center();
    g.click_pos(p);
    g.frames(3);
    assert_eq!(g.app().machine.active().settings.gweike.crafts.fibre[0], before);
    // One click puts the recipe on the slot open in the editor and the editor shows it.
    let p = use_on(&g, "Mild 2.0mm 1.5S F-2 O2").center();
    g.click_pos(p);
    g.frames(4);
    assert_eq!(g.app().machine.active().settings.gweike.crafts.fibre[0].cut.speed_mm_s, 33.0);
    assert!(g.has_text("Mild 2.0mm 1.5S F-2 O2 is on Fibre (1200 W) slot 0."), "{:?}", g.visible_texts());
    // The CO2 chip lists the CO2 recipes instead.
    g.click_label("CO2");
    g.frames(3);
    assert_eq!(g.find_texts("Birch 4mm air").len(), 2, "{:?}", g.visible_texts());
    assert!(g.find_texts("Mild 2.0mm 1.5S F-2 O2").is_empty());
    g.close_dialogs();
}
