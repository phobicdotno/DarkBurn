use darkburn_import::lbrn::{self, LbrnWriteOptions};
use darkburn_import::*;
use kurbo::{PathEl, Rect, Shape};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn rect_close(r: Rect, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    close(r.x0, x0) && close(r.y0, y0) && close(r.x1, x1) && close(r.y1, y1)
}

fn find<'a>(doc: &'a Imported, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<(&'a Item, &'a PathItem)> {
    doc.items.iter().find_map(|i| match &i.kind {
        ItemKind::Path(p) if rect_close(p.path.bounding_box(), x0, y0, x1, y1) => Some((i, p)),
        _ => None,
    })
}

fn path_at(doc: &Imported, x0: f64) -> Option<(&Item, &PathItem)> {
    doc.items.iter().find_map(|i| match &i.kind {
        ItemKind::Path(p) if close(p.path.bounding_box().x0, x0) => Some((i, p)),
        _ => None,
    })
}

#[test]
fn project_info_and_cut_settings() {
    let doc = lbrn::import(&data("project.lbrn2")).unwrap();
    let info = doc.project.as_ref().unwrap();
    assert_eq!(info.app_version.as_deref(), Some("1.4.04"));
    assert_eq!(info.format_version, Some(1));
    assert_eq!(info.material_height, Some(3.0));
    assert!(info.mirror_y && !info.mirror_x);
    assert_eq!(info.notes.as_deref(), Some("Use 3 mm birch\nSecond line"));
    assert!(info.show_notes_on_load);
    assert!(info.thumbnail_png.as_ref().is_some_and(|p| p.starts_with(b"\x89PNG")));
    assert_eq!(info.variable_text.get("Current").map(String::as_str), Some("5"));
    assert_eq!(info.ui_prefs.get("Optimize_ByGroup").map(String::as_str), Some("-1"));

    assert_eq!(doc.layer_settings.len(), 3);
    let cut = &doc.layer_settings[0];
    assert_eq!((cut.index, cut.name.as_str(), cut.mode.as_str()), (0, "Outline", "Cut"));
    assert_eq!(cut.speed(), Some(20.0));
    assert_eq!(cut.min_power(), Some(15.0));
    assert_eq!(cut.max_power(), Some(60.0));
    assert_eq!(cut.passes(), Some(2));
    assert_eq!(cut.priority(), Some(1));
    let scan = &doc.layer_settings[1];
    assert_eq!(scan.mode, "Scan");
    assert_eq!(scan.interval(), Some(0.1));
    assert_eq!(scan.angle(), Some(45.0));
    let img = &doc.layer_settings[2];
    assert_eq!(img.mode, "Image");
    assert_eq!(img.get("ditherMode"), Some("stucki"));
}

#[test]
fn shapes_v1() {
    let doc = lbrn::import(&data("project.lbrn2")).unwrap();

    let (rect_item, rect) = find(&doc, 40.0, 45.0, 60.0, 55.0).expect("rounded rect");
    assert_eq!(rect_item.layer.as_deref(), Some("Outline"));
    assert_eq!(rect_item.layer_index, Some(0));
    assert!(rect.path.elements().iter().any(|e| matches!(e, PathEl::CurveTo(..))), "corner radius");

    assert!(find(&doc, 95.0, 97.0, 105.0, 103.0).is_some(), "ellipse");

    let (path_item, path) = path_at(&doc, 0.0).expect("vertlist path");
    assert_eq!(path_item.layer.as_deref(), Some("Engrave"));
    assert_eq!(path.path.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count(), 1);
    assert!(matches!(path.path.elements().last(), Some(PathEl::ClosePath)));
    assert!(path.path.bounding_box().x1 > 10.0, "Bézier handle bulges right");

    // Second shape re-uses VertID/PrimID 0.
    let (_, copy) = path_at(&doc, 200.0).expect("shared geometry");
    assert_eq!(copy.path.elements().len(), path.path.elements().len());

    let (_, tri) = find(&doc, 300.0, 0.0, 305.0, 5.0).expect("LineClosed triangle");
    assert!(matches!(tri.path.elements().last(), Some(PathEl::ClosePath)));

    let (_, hex) = path_at(&doc, 390.0).expect("polygon");
    assert_eq!(hex.path.elements().iter().filter(|e| matches!(e, PathEl::LineTo(..))).count(), 5);
}

#[test]
fn groups_compose_transforms() {
    let doc = lbrn::import(&data("project.lbrn2")).unwrap();
    let (a, _) = find(&doc, 1000.0, 0.0, 1002.0, 2.0).expect("group child");
    let (b, _) = find(&doc, 1018.0, -2.0, 1022.0, 2.0).expect("nested group child, scaled 2");
    let ga = a.group.unwrap();
    let gb = b.group.unwrap();
    assert_ne!(ga, gb);
    assert_eq!(doc.groups.iter().find(|g| g.id == gb).unwrap().parent, Some(ga));
}

#[test]
fn text_with_backup_path_and_bitmap() {
    let doc = lbrn::import(&data("project.lbrn2")).unwrap();
    let text = doc
        .items
        .iter()
        .find_map(|i| match &i.kind {
            ItemKind::Text(t) => Some(t),
            _ => None,
        })
        .expect("text");
    assert_eq!(text.text, "Hi");
    assert_eq!(text.font, "Century Gothic");
    assert_eq!(text.h_align, HAlign::Center);
    assert!(close(text.height, 8.0));
    let o = text.outline.as_ref().expect("backup path");
    assert!(rect_close(o.bounding_box(), 0.0, 500.0, 4.0, 508.0));

    let img = doc
        .items
        .iter()
        .find_map(|i| match &i.kind {
            ItemKind::Image(im) => Some((i, im)),
            _ => None,
        })
        .expect("bitmap");
    assert_eq!(img.0.layer_index, Some(2));
    let c = img.1.corners();
    assert!(close(c[0].x, 595.0) && close(c[0].y, 595.0) && close(c[2].x, 605.0) && close(c[2].y, 605.0), "{c:?}");
    assert_eq!(img.1.image.to_rgb8().get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(img.1.attributes.get("DPI").map(String::as_str), Some("254"));
}

#[test]
fn legacy_v0_format() {
    let doc = lbrn::import(&data("legacy.lbrn")).unwrap();
    assert_eq!(doc.project.as_ref().unwrap().format_version, Some(0));
    let (item, p) = path_at(&doc, 10.0).expect("v0 path");
    assert_eq!(item.layer.as_deref(), Some("C03"));
    assert!(close(p.path.bounding_box().y0, 20.0));
    assert_eq!(p.path.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count(), 1);
    assert!(matches!(p.path.elements().last(), Some(PathEl::ClosePath)));
    assert!(find(&doc, -2.0, -2.0, 2.0, 2.0).is_some(), "rotated rect");
    assert_eq!(detect_format(None, &data("legacy.lbrn")), Some(Format::LightBurn));
}

#[test]
fn lbrn2_round_trip() {
    let doc = lbrn::import(&data("project.lbrn2")).unwrap();
    let text = lbrn::write_lbrn2(&doc, &LbrnWriteOptions::default());
    let back = lbrn::import(text.as_bytes()).unwrap();
    assert_eq!(back.items.len(), doc.items.len());
    assert_eq!(back.layer_settings.len(), doc.layer_settings.len());
    assert_eq!(back.layer_settings[0].speed(), Some(20.0));
    assert_eq!(back.groups.len(), doc.groups.len());
    for (a, b) in doc.items.iter().zip(&back.items) {
        let (ra, rb) = (a.bounds().unwrap(), b.bounds().unwrap());
        assert!(rect_close(rb, ra.x0, ra.y0, ra.x1, ra.y1), "{ra:?} vs {rb:?}");
        assert_eq!(a.layer_index, b.layer_index);
        assert_eq!(a.group.is_some(), b.group.is_some());
    }
    let info = back.project.unwrap();
    assert_eq!(info.notes.as_deref(), Some("Use 3 mm birch\nSecond line"));
    assert_eq!(info.variable_text.get("Current").map(String::as_str), Some("5"));
}
