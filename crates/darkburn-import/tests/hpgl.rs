use darkburn_import::hpgl::{self, HpglExportOptions, HpglOptions};
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

fn find(doc: &Imported, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<(&Item, &PathItem)> {
    doc.items.iter().find_map(|i| match &i.kind {
        ItemKind::Path(p) if rect_close(p.path.bounding_box(), x0, y0, x1, y1) => Some((i, p)),
        _ => None,
    })
}

#[test]
fn import_instructions() {
    let doc = hpgl::import(&data("sample.plt"), &HpglOptions::default()).unwrap();
    assert_eq!(doc.items.len(), 5, "{:#?}", doc.items.iter().map(|i| i.bounds()).collect::<Vec<_>>());

    let (sq_item, sq) = find(&doc, 0.0, 0.0, 10.0, 10.0).expect("PD square");
    assert_eq!(sq_item.layer.as_deref(), Some("Pen 1"));
    assert_eq!(sq.stroke, Some(Rgb::BLACK));
    assert!(matches!(sq.path.elements().last(), Some(PathEl::ClosePath)));

    let (c_item, _) = find(&doc, 15.0, 15.0, 25.0, 25.0).expect("CI circle");
    assert_eq!(c_item.layer.as_deref(), Some("Pen 2"));

    assert!(find(&doc, 25.0, 0.0, 26.0, 1.0).is_some(), "PR relative moves");
    assert!(find(&doc, 50.0, 0.0, 60.0, 10.0).is_some(), "AA arc");
    assert!(find(&doc, 0.0, 0.0, 60.0, 10.0).is_some(), "EA rectangle");
    assert!(doc.warnings.iter().any(|w| w.contains("LB")));
    assert_eq!(detect_format(None, &data("sample.plt")), Some(Format::Hpgl));
}

#[test]
fn precision_setting() {
    let doc = hpgl::import(&data("sample.plt"), &HpglOptions { units_per_mm: 4.0 }).unwrap();
    assert!(find(&doc, 0.0, 0.0, 100.0, 100.0).is_some());
}

#[test]
fn export_round_trip() {
    let doc = hpgl::import(&data("sample.plt"), &HpglOptions::default()).unwrap();
    let text = hpgl::export(&doc.items, &HpglExportOptions::default());
    assert!(text.starts_with("IN;"));
    assert!(text.contains("SP1;") && text.contains("SP2;"));
    let back = hpgl::import(text.as_bytes(), &HpglOptions::default()).unwrap();
    assert_eq!(back.items.len(), doc.items.len());
    for (a, b) in doc.items.iter().zip(&back.items) {
        let (ra, rb) = (a.bounds().unwrap(), b.bounds().unwrap());
        assert!((ra.x0 - rb.x0).abs() < 0.06 && (ra.y1 - rb.y1).abs() < 0.06, "{ra:?} {rb:?}");
    }
}

#[test]
fn garbage_is_an_error() {
    assert!(hpgl::import(b"hello world 123", &HpglOptions::default()).is_err());
}
