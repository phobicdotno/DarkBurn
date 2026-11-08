use darkburn_text::shx::write_shapes_1_0;
use darkburn_text::{HAlign, ShxFont, TextSpec};
use kurbo::{PathEl, Shape};

/// Tiny hand-made font, cap height 8. Shapes:
/// 'L': up 8, pen up, back down 8, pen down, right 4, pen up, right 2 (advance 6).
/// 'O': full octant circle radius 4 starting at the left point... then advance by moving right.
/// 'U': bulge semicircle of width 8 (127 = half circle).
/// 'S': calls sub-shape 200 twice.
/// 'D': divide by 2 then draw east 16 (-> 8 long) and a long-form displacement.
fn font() -> Vec<u8> {
    let l = vec![0x84, 2, 0x8C, 1, 0x40, 2, 0x20, 0];
    let o = vec![10, 4, 0x00, 2, 8, 12, 0, 0]; // circle, then pen up, move (+12,0)
    let u = vec![12, 8, 0, 127, 0];
    let s = vec![7, 200, 7, 200, 0];
    let d = vec![3, 2, 0x80, 0]; // divide by 2, then east 8 -> 4
    let sub = vec![0x40, 2, 0x20, 0]; // east 4, pen up, east 2
    write_shapes_1_0(
        "TESTFONT",
        8,
        2,
        &[(b'L' as u16, "L", l), (b'O' as u16, "O", o), (b'U' as u16, "U", u), (b'S' as u16, "S", s), (b'D' as u16, "D", d), (200, "SUB", sub)],
    )
}

fn pts(p: &kurbo::BezPath) -> Vec<(f64, f64)> {
    p.elements()
        .iter()
        .filter_map(|e| match e {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some((p.x, p.y)),
            _ => None,
        })
        .collect()
}

#[test]
fn parses_header_and_metrics() {
    let f = ShxFont::parse(&font()).unwrap();
    assert_eq!(f.name, "TESTFONT");
    assert_eq!(f.above, 8.0);
    assert_eq!(f.below, 2.0);
    assert!(f.has_glyph('L') && !f.has_glyph('Z'));
    assert_eq!(f.glyph_count(), 6);
}

#[test]
fn vector_codes_pen_up_and_advance() {
    let f = ShxFont::parse(&font()).unwrap();
    let (p, adv) = f.glyph_units('L').unwrap();
    assert_eq!(adv, 6.0);
    let els = p.elements();
    // Two open strokes: (0,0)-(0,8) and (0,0)-(4,0)
    assert_eq!(els.iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count(), 2);
    assert_eq!(pts(&p), vec![(0.0, 0.0), (0.0, 8.0), (0.0, 0.0), (4.0, 0.0)]);
    assert!(!els.iter().any(|e| matches!(e, PathEl::ClosePath)), "single-line fonts give open paths");
}

#[test]
fn octant_circle() {
    let f = ShxFont::parse(&font()).unwrap();
    let (p, adv) = f.glyph_units('O').unwrap();
    let b = p.bounding_box();
    assert!((b.x0 + 8.0).abs() < 0.01 && b.x1.abs() < 0.01, "{b:?}");
    assert!((b.y0 + 4.0).abs() < 0.01 && (b.y1 - 4.0).abs() < 0.01, "{b:?}");
    assert!((adv - 12.0).abs() < 1e-9);
}

#[test]
fn bulge_semicircle() {
    let f = ShxFont::parse(&font()).unwrap();
    let (p, adv) = f.glyph_units('U').unwrap();
    let b = p.bounding_box();
    assert!((adv - 8.0).abs() < 1e-9);
    assert!((b.y0 + 4.0).abs() < 0.01, "bulges below the chord: {b:?}");
    assert!(b.y1.abs() < 0.01);
    assert!((b.x0).abs() < 0.01 && (b.x1 - 8.0).abs() < 0.01);
}

#[test]
fn sub_shapes_and_scale_codes() {
    let f = ShxFont::parse(&font()).unwrap();
    let (p, adv) = f.glyph_units('S').unwrap();
    assert_eq!(adv, 12.0);
    assert_eq!(pts(&p), vec![(0.0, 0.0), (4.0, 0.0)], "second call runs with the pen still up");
    let (d, dadv) = f.glyph_units('D').unwrap();
    assert_eq!(dadv, 4.0);
    assert_eq!(pts(&d), vec![(0.0, 0.0), (4.0, 0.0)]);
}

#[test]
fn layout_scales_to_cap_height_and_aligns() {
    let f = ShxFont::parse(&font()).unwrap();
    let mut spec = TextSpec { text: "LL".into(), size: 16.0, ..Default::default() };
    let lay = f.layout(&spec);
    // scale = 16 / 8 = 2
    assert_eq!(lay.glyphs.len(), 2);
    assert!((lay.glyphs[1].origin_x - 12.0).abs() < 1e-9);
    assert!((lay.bounds.height() - 16.0).abs() < 1e-9);
    assert!((lay.width() - 24.0).abs() < 1e-9);
    spec.h_align = HAlign::Center;
    let c = f.layout(&spec);
    assert!((c.bounds.x0 - (lay.bounds.x0 - 12.0)).abs() < 1e-9);
    spec.letter_spacing = 1.0;
    spec.h_align = HAlign::Left;
    assert!((f.layout(&spec).width() - 26.0).abs() < 1e-9);
    // unknown character is reported and still advances
    let u = f.layout(&TextSpec { text: "Z".into(), size: 8.0, ..Default::default() });
    assert!(u.glyphs[0].missing);
}

#[test]
fn rejects_bad_files() {
    assert!(ShxFont::parse(b"").is_err());
    assert!(ShxFont::parse(b"not a shx file\x1a").is_err());
    assert!(ShxFont::parse(b"AutoCAD-86 bigfont 1.0\r\n\x1a\0\0").is_err());
    let mut data = font();
    data.truncate(data.len() - 5);
    assert!(ShxFont::parse(&data).is_err());
    let mut hdr = b"AutoCAD-86 shapes 1.0\r\n\x1a".to_vec();
    hdr.extend([0, 0, 5, 0, 200, 0]);
    assert!(ShxFont::parse(&hdr).is_err());
}

#[test]
fn runaway_sub_shape_recursion_terminates() {
    let rec = write_shapes_1_0("R", 8, 0, &[(b'A' as u16, "A", vec![7, b'A', 0x40, 0])]);
    let f = ShxFont::parse(&rec).unwrap();
    assert!(f.glyph_units('A').is_some());
}
