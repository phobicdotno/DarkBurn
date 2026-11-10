//! PDF import: geometry checked against numbers worked out by hand, and files built to trip the
//! reader (objects that name themselves, absurd sizes). The PDFs are written here, object by object.

use darkburn_import::pdf_import::{self, PdfOptions};
use darkburn_import::*;
use kurbo::{PathEl, Point, Rect, Shape};

const PT: f64 = 25.4 / 72.0;

/// A one-page PDF. `page` is put into the page dictionary, `content` is the page's content
/// stream, `extra` are the bodies of objects 5, 6, ...
fn pdf(page: &str, content: &str, extra: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.6\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    obj(&mut out, format!("<< /Type /Page /Parent 2 0 R /Contents 4 0 R {page} >>").as_bytes());
    obj(&mut out, stream("", content.as_bytes()).as_slice());
    for e in extra {
        obj(&mut out, e);
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in &offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< /Length {} {dict} >>\nstream\n", data.len()).into_bytes();
    v.extend(data);
    v.extend(b"\nendstream");
    v
}

fn read(bytes: &[u8]) -> Imported {
    pdf_import::import(bytes, &PdfOptions::default()).unwrap()
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

fn near(a: Point, x: f64, y: f64) -> bool {
    (a.x - x * PT).abs() < 1e-6 && (a.y - y * PT).abs() < 1e-6
}

fn box_pt(r: Rect, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let c = |a: f64, b: f64| (a - b * PT).abs() < 1e-6;
    c(r.x0, x0) && c(r.y0, y0) && c(r.x1, x1) && c(r.y1, y1)
}

#[test]
fn stacked_matrices_apply_innermost_first() {
    // (10,10) is scaled by (2,3) to (20,30), then moved by (10,20) to (30,50). (0,0) goes to (10,20).
    // After Q the matrix is gone again.
    let doc = read(&pdf("/MediaBox [0 0 300 300]", "q 1 0 0 1 10 20 cm 2 0 0 3 0 0 cm 0 0 m 10 10 l S Q 0 0 m 10 10 l S", &[]));
    let p = paths(&doc);
    assert_eq!(p.len(), 2);
    assert!(box_pt(p[0].path.bounding_box(), 10.0, 20.0, 30.0, 50.0), "{:?}", p[0].path.bounding_box());
    assert!(box_pt(p[1].path.bounding_box(), 0.0, 0.0, 10.0, 10.0), "{:?}", p[1].path.bounding_box());
    // A rotation by 90 degrees (cos 0, sin 1): (10,0) goes to (0,10), then to (100,10).
    let doc = read(&pdf("/MediaBox [0 0 300 300]", "1 0 0 1 100 0 cm 0 1 -1 0 0 0 cm 0 0 m 10 0 l S", &[]));
    let b = paths(&doc)[0].path.bounding_box();
    assert!(box_pt(b, 100.0, 0.0, 100.0, 10.0), "{b:?}");
}

#[test]
fn curve_short_forms_put_the_control_points_where_the_format_says() {
    let doc = read(&pdf("/MediaBox [0 0 300 300]", "10 10 m 20 30 40 10 v S 10 10 m 20 30 40 10 y S", &[]));
    let p = paths(&doc);
    // v: the first control point is the current point.
    match p[0].path.elements() {
        [PathEl::MoveTo(a), PathEl::CurveTo(c1, c2, e)] => {
            assert!(near(*a, 10.0, 10.0) && near(*c1, 10.0, 10.0) && near(*c2, 20.0, 30.0) && near(*e, 40.0, 10.0), "{:?}", p[0].path);
        }
        other => panic!("{other:?}"),
    }
    // y: the second control point is the end point.
    match p[1].path.elements() {
        [PathEl::MoveTo(a), PathEl::CurveTo(c1, c2, e)] => {
            assert!(near(*a, 10.0, 10.0) && near(*c1, 20.0, 30.0) && near(*c2, 40.0, 10.0) && near(*e, 40.0, 10.0), "{:?}", p[1].path);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_line_after_closing_starts_a_new_part_at_the_start_point() {
    // After `h` the current point is the start of the closed part; `l` begins a new part there.
    let doc = read(&pdf("/MediaBox [0 0 300 300]", "10 10 m 50 10 l 50 40 l h 10 80 l S  20 20 30 30 re 5 5 l S", &[]));
    let p = paths(&doc);
    assert_eq!(p.len(), 2);
    for path in &p {
        let els = path.path.elements();
        for (i, el) in els.iter().enumerate() {
            if matches!(el, PathEl::ClosePath) && i + 1 < els.len() {
                assert!(matches!(els[i + 1], PathEl::MoveTo(_)), "a part must start with a move: {els:?}");
            }
        }
    }
    match p[0].path.elements().last() {
        Some(PathEl::LineTo(e)) => assert!(near(*e, 10.0, 80.0)),
        other => panic!("{other:?}"),
    }
    let els = p[0].path.elements();
    assert!(matches!(els[els.len() - 2], PathEl::MoveTo(a) if near(a, 10.0, 10.0)), "{els:?}");
    let els = p[1].path.elements();
    assert!(matches!(els[els.len() - 2], PathEl::MoveTo(a) if near(a, 20.0, 20.0)), "{els:?}");
}

#[test]
fn a_form_inside_a_form_carries_every_matrix() {
    // Page: move by (100,0). Form 5 (matrix: scale 2) draws form 6 (matrix: move by (0,10)),
    // which draws a line from (0,0) to (10,0): on the page from (100,20) to (120,20).
    let f5 = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [2 0 0 2 0 0] /Resources << /XObject << /B 6 0 R >> >>", b"/B Do");
    let f6 = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 0 10]", b"0 0 m 10 0 l S");
    let doc = read(&pdf("/MediaBox [0 0 300 300] /Resources << /XObject << /A 5 0 R >> >>", "1 0 0 1 100 0 cm /A Do", &[f5, f6]));
    let b = paths(&doc)[0].path.bounding_box();
    assert!(box_pt(b, 100.0, 20.0, 120.0, 20.0), "{b:?}");
}
