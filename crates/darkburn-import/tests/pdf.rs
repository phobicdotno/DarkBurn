use darkburn_import::pdf_import::{self, PageSelection, PdfOptions};
use darkburn_import::*;
use kurbo::{Point, Rect, Shape};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

const PT: f64 = 25.4 / 72.0;

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

fn images(doc: &Imported) -> Vec<&ImageItem> {
    doc.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Image(im) => Some(im),
            _ => None,
        })
        .collect()
}

#[test]
fn vector_paths_transforms_and_colours() {
    let doc = pdf_import::import(&data("vector.pdf"), &PdfOptions::default()).unwrap();
    assert_eq!(doc.page_count, Some(2));

    // q 2 0 0 2 0 0 cm, 36 pt square -> 72 pt = 25.4 mm, red stroke.
    let (_, sq) = find(&doc, 0.0, 0.0, 25.4, 25.4).expect("scaled square");
    assert_eq!(sq.stroke, Some(Rgb(255, 0, 0)));
    assert_eq!(sq.fill, None);

    // CMYK fill (0 1 1 0) is red.
    let (_, tri) = find(&doc, 100.0 * PT, 100.0 * PT, 172.0 * PT, 172.0 * PT).expect("cmyk triangle");
    assert_eq!(tri.fill, Some(Rgb(255, 0, 0)));
    assert_eq!(tri.stroke, None);

    // Gray fill.
    let gray = doc.items.iter().find_map(|i| match &i.kind {
        ItemKind::Path(p) if p.fill == Some(Rgb(128, 128, 128)) => Some(p),
        _ => None,
    });
    assert!(gray.is_some(), "gray filled curve");

    // Dash pattern scaled to mm.
    let (_, dashed) = find(&doc, 300.0 * PT, 300.0 * PT, 372.0 * PT, 300.0 * PT).expect("dashed line");
    let d = dashed.dash.as_ref().expect("dash");
    assert!(close(d[0], 3.0 * PT) && close(d[1], 2.0 * PT));

    // Form XObject with its own matrix (translate 100 pt).
    let (_, form) = find(&doc, 100.0 * PT, 0.0, 172.0 * PT, 0.0).expect("form line");
    assert_eq!(form.stroke, Some(Rgb(0, 0, 255)));

    // Clip path noted, text skipped with a warning.
    assert!(doc.warnings.iter().any(|w| w.contains("Clipping")), "{:?}", doc.warnings);
    assert!(doc.warnings.iter().any(|w| w.contains("text")), "{:?}", doc.warnings);
}

#[test]
fn optional_content_layers() {
    let doc = pdf_import::import(&data("vector.pdf"), &PdfOptions::default()).unwrap();
    let (on, _) = find(&doc, 400.0 * PT, 400.0 * PT, 472.0 * PT, 400.0 * PT).expect("visible layer");
    assert_eq!(on.layer.as_deref(), Some("Cut lines"));
    assert!(!on.hidden);
    let (off, _) = find(&doc, 500.0 * PT, 500.0 * PT, 572.0 * PT, 500.0 * PT).expect("hidden layer");
    assert_eq!(off.layer.as_deref(), Some("Hidden stuff"));
    assert!(off.hidden);

    let without = pdf_import::import(&data("vector.pdf"), &PdfOptions { include_hidden_layers: false, ..Default::default() }).unwrap();
    assert!(find(&without, 500.0 * PT, 500.0 * PT, 572.0 * PT, 500.0 * PT).is_none());
}

#[test]
fn page_selection_and_rotation() {
    let p2 = pdf_import::import(&data("vector.pdf"), &PdfOptions { pages: PageSelection::Page(2), ..Default::default() }).unwrap();
    assert_eq!(p2.items.len(), 1);
    // Rotate 90 on a 200 x 100 pt page: (x, y) -> (y, 200 - x).
    let ItemKind::Path(p) = &p2.items[0].kind else { panic!() };
    let first = p.path.elements()[0].end_point().unwrap();
    assert!(close(first.x, 0.0) && close(first.y, 200.0 * PT), "{first:?}");

    let all = pdf_import::import(&data("vector.pdf"), &PdfOptions { pages: PageSelection::All, ..Default::default() }).unwrap();
    assert_eq!(all.groups.len(), 2);
    assert_eq!(all.groups[1].name.as_deref(), Some("Page 2"));

    assert!(pdf_import::import(&data("vector.pdf"), &PdfOptions { pages: PageSelection::Page(9), ..Default::default() }).is_err());
    assert_eq!(pdf_import::page_count(&data("vector.pdf")).unwrap(), 2);
}

#[test]
fn image_kinds() {
    let doc = pdf_import::import(&data("images.pdf"), &PdfOptions::default()).unwrap();
    let imgs = images(&doc);
    assert_eq!(imgs.len(), 7, "warnings: {:?}", doc.warnings);

    // Flate RGB, placed on a 72 pt square at the origin.
    let rgb = imgs[0].image.to_rgb8();
    assert_eq!(rgb.dimensions(), (2, 2));
    assert_eq!(rgb.get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(rgb.get_pixel(0, 1).0, [0, 0, 255]);
    let c = imgs[0].corners();
    assert!(close(c[2].x, 25.4) && close(c[2].y, 25.4));
    assert_eq!(c[0], Point::ZERO);

    // 1-bit gray.
    let bits = imgs[1].image.to_luma8();
    assert_eq!(bits.dimensions(), (8, 2));
    assert_eq!(bits.get_pixel(0, 0).0, [255]);
    assert_eq!(bits.get_pixel(1, 0).0, [0]);
    assert_eq!(bits.get_pixel(1, 1).0, [255]);

    // Stencil mask painted in the fill colour (red).
    let mask = imgs[2].image.to_rgba8();
    assert_eq!(mask.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(mask.get_pixel(7, 0).0[3], 0);

    // Indexed palette.
    let idx = imgs[3].image.to_rgb8();
    assert_eq!(idx.get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(idx.get_pixel(1, 0).0, [0, 255, 0]);

    // JPEG.
    let jpg = imgs[4].image.to_rgb8();
    assert_eq!(jpg.dimensions(), (4, 4));
    let p = jpg.get_pixel(1, 1).0;
    assert!(p[2] > 200 && p[0] < 40 && p[1] < 40, "{p:?}");

    // CCITT G4: left half black.
    let g4 = imgs[5].image.to_luma8();
    assert_eq!(g4.dimensions(), (16, 4));
    assert_eq!(g4.get_pixel(0, 0).0, [0]);
    assert_eq!(g4.get_pixel(15, 3).0, [255]);

    // Uncompressed inline image.
    let inline = imgs[6].image.to_rgb8();
    assert_eq!(inline.get_pixel(0, 0).0, [255, 0, 0]);
    assert_eq!(inline.get_pixel(1, 0).0, [0, 0, 255]);
}

#[test]
fn illustrator_files() {
    let ai = import(Format::Ai, &data("pdf_compatible.ai"), &ImportOptions::default()).unwrap();
    assert!(find(&ai, 0.0, 0.0, 25.4, 25.4).is_some());
    assert_eq!(detect_format(Some("logo.ai"), &data("pdf_compatible.ai")), Some(Format::Ai));
    assert_eq!(detect_format(None, &data("vector.pdf")), Some(Format::Pdf));

    let legacy = import(Format::Ai, &data("legacy_postscript.ai"), &ImportOptions::default());
    match legacy {
        Err(ImportError::Unsupported(msg)) => assert!(msg.contains("PDF Compatible"), "{msg}"),
        other => panic!("expected unsupported, got {other:?}"),
    }
}
