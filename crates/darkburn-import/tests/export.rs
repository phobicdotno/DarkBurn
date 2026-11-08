use std::collections::BTreeMap;

use darkburn_import::dxf_export::{self, CurveMode, DxfExportOptions, DxfVersion, LayerNaming};
use darkburn_import::geom::circle_path;
use darkburn_import::svg_export::{self, SvgExportOptions, SvgOrigin};
use darkburn_import::*;
use image::{DynamicImage, RgbaImage};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

fn rect_close(a: Rect, b: Rect, tol: f64) -> bool {
    close(a.x0, b.x0, tol) && close(a.y0, b.y0, tol) && close(a.x1, b.x1, tol) && close(a.y1, b.y1, tol)
}

fn sample() -> Vec<Item> {
    let mut sq = BezPath::new();
    sq.move_to((0.0, 0.0));
    sq.line_to((10.0, 0.0));
    sq.line_to((10.0, 10.0));
    sq.line_to((0.0, 10.0));
    sq.close_path();
    let mut wave = BezPath::new();
    wave.move_to((20.0, 0.0));
    wave.curve_to((25.0, 10.0), (30.0, -10.0), (35.0, 0.0));
    let mut img = RgbaImage::new(2, 1);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    img.put_pixel(1, 0, image::Rgba([0, 0, 255, 128]));
    vec![
        Item::path(sq, Some(Rgb(255, 0, 0)), None).with_layer("Cut"),
        Item::path(circle_path(Point::new(50.0, 50.0), 5.0), Some(Rgb(0, 0, 255)), None).with_layer("Score"),
        Item::path(wave, Some(Rgb(0, 0, 255)), Some(Rgb(0, 255, 0))).with_layer("Score"),
        Item::new(ItemKind::Image(ImageItem {
            image: DynamicImage::ImageRgba8(img),
            transform: Affine::translate((60.0, 0.0)) * Affine::scale_non_uniform(20.0, 10.0),
            attributes: BTreeMap::new(),
        }))
        .with_layer("Photo"),
    ]
}

fn paths(doc: &Imported) -> Vec<&PathItem> {
    doc.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Path(p) => Some(p),
            _ => None,
        })
        .collect()
}

#[test]
fn svg_export_layers_colours_images() {
    let svg = svg_export::export(&sample(), &SvgExportOptions::default());
    // Page = bounds: x 0..80, y -2.887 (the wave's dip) .. 55 (the circle's top).
    assert!(svg.contains("viewBox=\"0 0 80 57.88"), "{svg}");
    assert!(svg.contains("inkscape:label=\"Cut\"") && svg.contains("inkscape:label=\"Score\"") && svg.contains("inkscape:label=\"Photo\""));
    assert!(svg.contains("stroke:#ff0000"));
    assert!(svg.contains("fill:#00ff00"));
    assert!(svg.contains("data:image/png;base64,"));
    // Y flip: the square's bottom-left corner (0, 0) sits 55 mm below the page top.
    assert!(svg.contains("d=\"M0 55.000"), "{svg}");

    let page = svg_export::export(&sample(), &SvgExportOptions { origin: SvgOrigin::Page { width: 100.0, height: 100.0 }, ..Default::default() });
    assert!(page.contains("width=\"100mm\"") && page.contains("M0 100 L10 100"));
}

#[test]
fn ai_export_round_trips_through_pdf_reader() {
    let items = sample();
    let out = ai_export::export(&items, &ai_export::AiExportOptions::default());
    assert!(out.bytes.starts_with(b"%PDF"));
    let back = import(Format::Ai, &out.bytes, &ImportOptions::default()).unwrap();
    let ps = paths(&back);
    assert_eq!(ps.len(), 3);
    // The page starts at the artwork's bottom-left corner.
    let shift = Affine::translate((0.0, -bounds_of(items.iter()).unwrap().y0));
    for (orig, got) in items.iter().take(3).zip(back.items.iter()) {
        let want = shift.transform_rect_bbox(orig.bounds().unwrap());
        assert!(rect_close(want, got.bounds().unwrap(), 0.01), "{want:?} vs {:?}", got.bounds());
        assert_eq!(orig.layer, got.layer, "layers survive as optional content");
        assert_eq!(orig.colour(), got.colour());
    }
    let img = back
        .items
        .iter()
        .find_map(|i| match &i.kind {
            ItemKind::Image(im) => Some(im),
            _ => None,
        })
        .expect("image");
    let px = img.image.to_rgba8();
    assert_eq!(px.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(px.get_pixel(1, 0).0, [0, 0, 255, 128], "alpha via soft mask");
    let c = img.corners();
    assert!(close(c[0].x, 60.0, 0.01) && close(c[2].x, 80.0, 0.01) && close(c[2].y, 10.0 + 2.8868, 0.01), "{c:?}");
}

#[test]
fn dxf_export_arcs_circles_layers() {
    let items = sample();
    let out = dxf_export::export(&items, &DxfExportOptions::default());
    assert!(out.warnings.iter().any(|w| w.contains("images")));
    let text = String::from_utf8_lossy(&out.bytes);
    assert!(text.contains("CIRCLE"), "circle written as CIRCLE");
    assert!(text.contains("LWPOLYLINE"));
    assert!(text.contains("C_FF0000") && text.contains("C_0000FF"), "layer per colour");

    let back = darkburn_import::dxf_import::import(&out.bytes, &Default::default()).unwrap();
    let ps = paths(&back);
    assert_eq!(ps.len(), 3);
    for orig in items.iter().take(3) {
        let ob = orig.bounds().unwrap();
        assert!(ps.iter().any(|p| rect_close(p.path.bounding_box(), ob, 0.02)), "missing {ob:?}");
    }
    let red = ps.iter().find(|p| p.stroke == Some(Rgb(255, 0, 0))).expect("red layer colour");
    assert!(matches!(red.path.elements().last(), Some(PathEl::ClosePath)));
}

#[test]
fn dxf_export_segments_and_item_layers_r12() {
    let opts = DxfExportOptions {
        curves: CurveMode::Segments,
        circles: false,
        layers: LayerNaming::ByItemLayer,
        version: DxfVersion::R12,
        decimals: Some(2),
        ..Default::default()
    };
    let out = dxf_export::export(&sample(), &opts);
    let text = String::from_utf8_lossy(&out.bytes);
    assert!(text.contains("POLYLINE") && !text.contains("LWPOLYLINE"));
    assert!(!text.contains("CIRCLE"));
    assert!(text.contains("\nScore\r\n") || text.contains("\nScore\n"));
    let back = darkburn_import::dxf_import::import(&out.bytes, &Default::default()).unwrap();
    let ps = paths(&back);
    assert_eq!(ps.len(), 3);
    assert!(ps.iter().all(|p| p.path.elements().iter().all(|e| !matches!(e, PathEl::CurveTo(..)))), "segments only");
    let circle = ps.iter().find(|p| rect_close(p.path.bounding_box(), Rect::new(45.0, 45.0, 55.0, 55.0), 0.05)).expect("circle as segments");
    assert!(circle.path.elements().len() > 20);
}

#[test]
fn dxf_layer_names_are_sanitised() {
    assert_eq!(dxf_export::sanitize_layer_name("a/b:c"), "a_b_c");
    assert_eq!(dxf_export::sanitize_layer_name("  "), "0");
}

#[test]
fn signcut_is_reported_unsupported() {
    let bytes = std::fs::read(format!("{}/tests/data/design.sc", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert_eq!(detect_format(Some("design.sc"), &bytes), Some(Format::SignCut));
    assert!(matches!(import_auto(Some("design.sc"), &bytes, &ImportOptions::default()), Err(ImportError::Unsupported(_))));
}

#[test]
fn hidden_items_are_not_exported() {
    let mut items = sample();
    for i in &mut items {
        i.hidden = true;
    }
    let svg = svg_export::export(&items, &SvgExportOptions::default());
    assert!(!svg.contains("<path"));
    let dxf = dxf_export::export(&items, &DxfExportOptions::default());
    assert!(!String::from_utf8_lossy(&dxf.bytes).contains("LWPOLYLINE"));
    let _ = Shape::area(&Rect::ZERO);
}
