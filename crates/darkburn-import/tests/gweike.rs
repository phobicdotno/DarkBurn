use std::collections::BTreeMap;

use darkburn_import::geom::circle_path;
use darkburn_import::gweike::{self, GweikeDxfOptions, NcOptions};
use darkburn_import::*;
use kurbo::{BezPath, Point};

fn items() -> Vec<Item> {
    let mut sq = BezPath::new();
    sq.move_to((1.23456, 0.0));
    sq.line_to((10.0, 0.0));
    sq.line_to((10.0, 10.0));
    sq.line_to((0.0, 10.0));
    sq.close_path();
    let mut line = BezPath::new();
    line.move_to((100.0, 0.0));
    line.line_to((120.0, 0.0));
    vec![
        Item::path(sq, Some(Rgb(255, 0, 0)), None).with_layer("Cut"),
        Item::path(circle_path(Point::new(50.0, 50.0), 5.0), Some(Rgb(0, 0, 255)), None),
        Item::path(line, Some(Rgb(0, 0, 255)), None).with_layer("Other"),
    ]
}

fn materials() -> GweikeDxfOptions {
    let mut m = BTreeMap::new();
    m.insert("Cut".to_string(), "Steel 2mm".to_string());
    m.insert("#0000FF".to_string(), "Acrylic 3mm".to_string());
    GweikeDxfOptions { materials: m, ..Default::default() }
}

#[test]
fn dxf_one_layer_per_material() {
    let out = gweike::export_dxf_per_material(&items(), &materials());
    let text = String::from_utf8_lossy(&out.bytes);
    assert!(text.contains("Steel 2mm") && text.contains("Acrylic 3mm"));
    assert!(text.contains("AC1009"), "R12 by default");
    assert!(text.contains("1.23\r\n") || text.contains("1.23\n"), "0.01 mm precision");
    assert!(!text.contains("1.2345"));

    let back = dxf_import::import(&out.bytes, &Default::default()).unwrap();
    let mut layers: Vec<String> = back.items.iter().filter_map(|i| i.layer.clone()).collect();
    layers.sort();
    layers.dedup();
    assert_eq!(layers, vec!["Acrylic 3mm".to_string(), "Steel 2mm".to_string()]);
}

#[test]
fn material_rules() {
    let opts = materials();
    let it = items();
    assert_eq!(gweike::material_of(&it[0], &opts), "Steel 2mm", "by layer name");
    assert_eq!(gweike::material_of(&it[1], &opts), "Acrylic 3mm", "by colour");
    assert_eq!(gweike::material_of(&it[2], &GweikeDxfOptions::default()), "Other", "falls back to the item layer");
}

#[test]
fn nc_gcode() {
    let nc = gweike::export_nc(&items(), &NcOptions { materials: materials(), ..Default::default() });
    assert!(nc.contains("G21\nG90\n"));
    assert!(nc.trim_end().ends_with("M30"));
    assert_eq!(nc.matches("M07").count(), 3, "{nc}");
    assert_eq!(nc.matches("M08").count(), 3);
    assert!(nc.contains("G03"), "the counter-clockwise circle uses G03 arcs:\n{nc}");
    assert!(nc.contains("(MATERIAL: Steel 2mm)") && nc.contains("(MATERIAL: Acrylic 3mm)"));
    // Geometry only: no spindle/laser power, no feed unless asked for.
    let code: Vec<&str> = nc.lines().filter(|l| !l.starts_with('(')).collect();
    for bad in ["M03", "M04", " S", " F"] {
        assert!(code.iter().all(|l| !l.contains(bad)), "unexpected {bad:?} in\n{nc}");
    }
    // Every contour starts with a rapid to its first point.
    assert!(nc.contains("G00 X100 Y0\nM07\nG01 X120 Y0\nM08"));

    let with_feed = gweike::export_nc(&items(), &NcOptions { feed: Some(1200.0), arcs: false, ..Default::default() });
    assert!(with_feed.contains(" F1200"));
    assert!(!with_feed.contains("G02") && !with_feed.contains("G03"));
    let custom = gweike::export_nc(&items(), &NcOptions { cut_on: "M10".into(), cut_off: "M11".into(), ..Default::default() });
    assert_eq!(custom.matches("M10").count(), 3);
}

#[test]
fn circle_arcs_close_on_start() {
    let c = vec![Item::path(circle_path(Point::new(0.0, 0.0), 10.0), Some(Rgb::BLACK), None)];
    let nc = gweike::export_nc(&c, &NcOptions::default());
    let last_arc = nc.lines().filter(|l| l.starts_with("G03")).last().unwrap();
    assert!(last_arc.starts_with("G03 X10 Y0 "), "{nc}");
}
