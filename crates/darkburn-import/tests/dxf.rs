use darkburn_import::dxf_import::{self, DxfOptions};
use darkburn_import::*;
use kurbo::{ParamCurve, PathEl, Point, Rect, Shape};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn rect_close(r: Rect, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    close(r.x0, x0) && close(r.y0, y0) && close(r.x1, x1) && close(r.y1, y1)
}

fn paths(doc: &Imported) -> Vec<(&Item, &PathItem)> {
    doc.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Path(p) => Some((i, p)),
            _ => None,
        })
        .collect()
}

fn texts(doc: &Imported) -> Vec<&TextItem> {
    doc.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn find_by_bounds(doc: &Imported, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<(&Item, &PathItem)> {
    paths(doc).into_iter().find(|(_, p)| rect_close(p.path.bounding_box(), x0, y0, x1, y1))
}

#[test]
fn entities_layers_and_colours() {
    let doc = dxf_import::import(&data("entities.dxf"), &DxfOptions::default()).unwrap();

    // LINE on layer CUT (ACI 1 = red).
    let (item, line) = find_by_bounds(&doc, 0.0, 0.0, 10.0, 0.0).expect("line");
    assert_eq!(item.layer.as_deref(), Some("CUT"));
    assert_eq!(line.stroke, Some(Rgb(255, 0, 0)));

    // Bulged LWPOLYLINE: half circle dipping to y = -5, closed.
    let (_, poly) = find_by_bounds(&doc, 20.0, -5.0, 30.0, 0.0).expect("bulge polyline");
    assert!(matches!(poly.path.elements().last(), Some(PathEl::ClosePath)));
    let lowest =
        poly.path.segments().flat_map(|s| (0..=20).map(move |k| s.eval(f64::from(k) / 20.0))).any(|p| (p - Point::new(25.0, -5.0)).hypot() < 1e-3);
    assert!(lowest, "arc passes through (25,-5)");

    // CIRCLE on a layer with a true colour.
    let (circle_item, circle) = find_by_bounds(&doc, 45.0, 45.0, 55.0, 55.0).expect("circle");
    assert_eq!(circle_item.layer.as_deref(), Some("TRUE"));
    assert_eq!(circle.stroke, Some(Rgb(0x12, 0x34, 0x56)));

    // ARC with its own colour (ACI 5 = blue).
    let (_, arc) = find_by_bounds(&doc, 0.0, 50.0, 10.0, 60.0).expect("arc");
    assert_eq!(arc.stroke, Some(Rgb(0, 0, 255)));

    // Full ELLIPSE.
    assert!(find_by_bounds(&doc, 80.0, -10.0, 120.0, 10.0).is_some(), "ellipse");

    // Cubic SPLINE with two spans -> two Béziers from (0,100) to (40,100).
    let (_, spline) = paths(&doc).into_iter().find(|(_, p)| p.path.elements()[0] == PathEl::MoveTo(Point::new(0.0, 100.0))).expect("spline");
    assert_eq!(spline.path.elements().iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count(), 2);
    let end = spline.path.elements().last().unwrap().end_point().unwrap();
    assert!(close(end.x, 40.0) && close(end.y, 100.0));

    // Dashed line type is noted.
    let (_, dashed) = find_by_bounds(&doc, 0.0, 300.0, 10.0, 300.0).expect("dashed");
    assert_eq!(dashed.dash.as_deref(), Some(&[0.5, 0.25][..]));
    assert!(doc.warnings.iter().any(|w| w.contains("Dashed")));

    // Frozen and switched-off layers are left out.
    assert!(paths(&doc).iter().all(|(i, _)| !matches!(i.layer.as_deref(), Some("FROZEN" | "OFF"))));
    assert!(doc.warnings.iter().any(|w| w.contains("hidden or frozen")), "{:?}", doc.warnings);
}

#[test]
fn blocks_nested_inserts_and_attributes() {
    let doc = dxf_import::import(&data("entities.dxf"), &DxfOptions::default()).unwrap();
    // OUTER at (200,200) rotated 90 deg, containing INNER scaled 2 at the OUTER base point.
    let (item, sq) = find_by_bounds(&doc, 198.0, 200.0, 200.0, 202.0).expect("nested block square");
    assert_eq!(item.layer.as_deref(), Some("CUT"), "layer-0 entities take the insert's layer");
    assert_eq!(sq.stroke, Some(Rgb(255, 0, 0)), "ByBlock colour comes from the insert");
    let g = item.group.expect("in a group");
    let parent = doc.groups.iter().find(|x| x.id == g).unwrap().parent;
    assert!(parent.is_some(), "nested groups");

    let attr = texts(&doc).into_iter().find(|t| t.text == "PART-7").expect("attribute text");
    let o = attr.transform * Point::ZERO;
    assert!(close(o.x, 210.0) && close(o.y, 190.0));
    assert!(texts(&doc).iter().all(|t| t.text != "x"), "ATTDEF is not drawn");
}

#[test]
fn text_and_mtext() {
    let doc = dxf_import::import(&data("entities.dxf"), &DxfOptions::default()).unwrap();
    let t = texts(&doc).into_iter().find(|t| t.text.starts_with("Hello")).expect("text");
    assert_eq!(t.text, "Hello °");
    assert_eq!(t.font, "arial.ttf");
    assert_eq!(t.style.as_deref(), Some("ARIALSTYLE"));
    assert!(close(t.height, 3.5));
    let x = t.transform * Point::new(1.0, 0.0) - t.transform * Point::ZERO;
    assert!(close(x.x, 0.0) && close(x.y, 1.0), "rotated 90 degrees: {x:?}");

    let m = texts(&doc).into_iter().find(|t| t.text.starts_with("Line one")).expect("mtext");
    assert_eq!(m.text, "Line one\nLine two");
    assert_eq!(m.v_align, VAlign::Top);
    assert_eq!(m.h_align, HAlign::Left);
}

#[test]
fn auto_join() {
    let off = dxf_import::import(&data("entities.dxf"), &DxfOptions::default()).unwrap();
    let near = |d: &Imported| paths(d).into_iter().filter(|(_, p)| p.path.bounding_box().x0 >= 499.0).count();
    assert_eq!(near(&off), 2);
    let on = dxf_import::import(&data("entities.dxf"), &DxfOptions { auto_join_tolerance: Some(0.01), ..Default::default() }).unwrap();
    assert_eq!(near(&on), 1);
}

#[test]
fn units() {
    let len = |name: &str, opts: &DxfOptions| {
        let d = dxf_import::import(&data(name), opts).unwrap();
        let (_, p) = paths(&d)[0];
        (p.path.bounding_box().width(), d.warnings.clone())
    };
    let o = DxfOptions::default();
    assert!(close(len("units_inches.dxf", &o).0, 25.4));
    assert!(close(len("units_centimetres.dxf", &o).0, 10.0));
    assert!(close(len("units_metres.dxf", &o).0, 1000.0));
    assert!(close(len("units_kilometres.dxf", &o).0, 1_000_000.0));
    assert!(close(len("units_measurement_english.dxf", &o).0, 25.4));
    let (w, warnings) = len("units_none.dxf", &o);
    assert!(close(w, 1.0));
    assert!(warnings.iter().any(|w| w.contains("units")));
    let (w, _) = len("units_none.dxf", &DxfOptions { fallback_mm_per_unit: 25.4, ..Default::default() });
    assert!(close(w, 25.4));
    let (w, _) = len("units_inches.dxf", &DxfOptions { force_mm_per_unit: Some(1.0), ..Default::default() });
    assert!(close(w, 1.0));
    assert_eq!(dxf_import::units_to_mm(7), Some(1_000_000.0));
}

#[test]
fn minimal_entities_only() {
    let d = dxf_import::import(&data("minimal.dxf"), &DxfOptions::default()).unwrap();
    let (_, p) = paths(&d)[0];
    assert!(close(p.path.bounding_box().width(), 3.0));
    assert!(close(p.path.bounding_box().height(), 4.0));
}

#[test]
fn aci_palette() {
    assert_eq!(dxf_import::aci_to_rgb(1), Rgb(255, 0, 0));
    assert_eq!(dxf_import::aci_to_rgb(7), Rgb::BLACK);
    assert_eq!(dxf_import::nearest_aci(Rgb(255, 0, 0)), 1);
    assert_eq!(dxf_import::nearest_aci(Rgb(0, 0, 255)), 5);
    assert_eq!(dxf_import::nearest_aci(Rgb::BLACK), 7);
}

#[test]
fn mtext_formatting() {
    assert_eq!(dxf_import::strip_mtext("{\\H2.5;\\C1;Big}\\Ptext \\S1/2;"), "Big\ntext 1/2");
    assert_eq!(dxf_import::decode_text_codes("50%%p1 %%c10 %%%"), "50±1 Ø10 %");
}

#[test]
fn detect() {
    assert_eq!(detect_format(Some("x.dxf"), &data("entities.dxf")), Some(Format::Dxf));
    assert_eq!(detect_format(None, &data("entities.dxf")), Some(Format::Dxf));
}
