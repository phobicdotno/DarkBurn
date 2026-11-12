mod common;
use common::*;
use darkburn_text::*;
use kurbo::{PathEl, Shape};

fn spec(text: &str, family: &str) -> TextSpec {
    TextSpec { text: text.into(), family: family.into(), size: 10.0, ..Default::default() }
}

fn moves(paths: &[kurbo::BezPath]) -> usize {
    paths.iter().flat_map(|p| p.elements().iter()).filter(|e| matches!(e, PathEl::MoveTo(_))).count()
}

#[test]
fn families_are_listed() {
    let l = lib();
    let f = l.families();
    assert!(f.contains(&MONO.to_string()) && f.contains(&SANS.to_string()), "{f:?}");
    assert!(l.has_family("ibm plex mono"));
    assert!(!l.has_family("Nope Sans"));
}

#[test]
fn cap_height_mode_makes_capitals_the_requested_height() {
    let l = lib();
    let lay = layout_text(&spec("H", MONO), &l).unwrap();
    assert_eq!(lay.glyphs.len(), 1);
    let b = lay.bounds;
    assert!((b.height() - 10.0).abs() < 0.2, "cap H is {} tall", b.height());
    assert!((lay.metrics.cap_height_mm - 10.0).abs() < 1e-6);
}

#[test]
fn em_mode_scales_the_em_square() {
    let l = lib();
    let mut s = spec("H", MONO);
    s.size_mode = SizeMode::Em;
    let lay = layout_text(&s, &l).unwrap();
    assert!((lay.metrics.em_mm - 10.0).abs() < 1e-9);
    assert!(lay.bounds.height() < 8.0 && lay.bounds.height() > 5.0);
}

#[test]
fn monospace_advance_and_letter_spacing() {
    let l = lib();
    let a = layout_text(&spec("iii", MONO), &l).unwrap();
    let adv = a.glyphs[0].advance;
    assert!(a.glyphs.iter().all(|g| (g.advance - adv).abs() < 1e-9));
    let mut s = spec("iii", MONO);
    s.letter_spacing = 2.0;
    let b = layout_text(&s, &l).unwrap();
    assert!((b.width() - a.width() - 6.0).abs() < 1e-6);
    assert!((b.glyphs[1].origin_x - b.glyphs[0].origin_x - (adv + 2.0)).abs() < 1e-6);
}

#[test]
fn horizontal_alignment() {
    let l = lib();
    let mut s = spec("abc", MONO);
    let left = layout_text(&s, &l).unwrap();
    s.h_align = HAlign::Center;
    let c = layout_text(&s, &l).unwrap();
    s.h_align = HAlign::Right;
    let r = layout_text(&s, &l).unwrap();
    let w = left.width();
    assert!((c.lines[0].x_start + w / 2.0).abs() < 1e-9);
    assert!((r.lines[0].x_start + w).abs() < 1e-9);
    assert!((c.bounds.x0 - left.bounds.x0 + w / 2.0).abs() < 1e-6);
}

#[test]
fn vertical_alignment_and_multiline() {
    let l = lib();
    let mut s = spec("H\nH", MONO);
    let bottom = layout_text(&s, &l).unwrap();
    assert_eq!(bottom.lines.len(), 2);
    assert!(bottom.lines[1].baseline_y.abs() < 1e-9, "last baseline at 0");
    assert!(bottom.lines[0].baseline_y > 0.0);
    let lh = bottom.metrics.line_height_mm;
    assert!((bottom.lines[0].baseline_y - lh).abs() < 1e-9);
    s.v_align = VAlign::Top;
    let top = layout_text(&s, &l).unwrap();
    assert!((top.bounds.y1).abs() < 0.05, "cap line of first line at 0, got {}", top.bounds.y1);
    s.v_align = VAlign::Middle;
    let mid = layout_text(&s, &l).unwrap();
    assert!((mid.bounds.center().y).abs() < 0.2);
    s.line_spacing = 5.0;
    let wide = layout_text(&s, &l).unwrap();
    assert!((wide.metrics.line_height_mm - lh - 5.0).abs() < 1e-9);
}

#[test]
fn upper_case_option() {
    let l = lib();
    let mut s = spec("ab", MONO);
    s.upper_case = true;
    let lay = layout_text(&s, &l).unwrap();
    assert_eq!(lay.glyphs[0].text, "A");
    // "A" is cap height tall, "a" would be x-height tall
    assert!((lay.bounds.height() - 10.0).abs() < 0.3);
}

#[test]
fn variable_font_gives_real_bold() {
    let l = lib();
    let r = l.resolve(SANS, true, false).unwrap();
    assert_eq!(r.weight_axis, Some(700.0));
    assert!(!r.synthetic_bold);
    let reg = layout_text(&spec("Hamburg", SANS), &l).unwrap();
    let mut s = spec("Hamburg", SANS);
    s.bold = true;
    let bold = layout_text(&s, &l).unwrap();
    assert!(bold.width() > reg.width() + 0.1, "bold {} regular {}", bold.width(), reg.width());
}

#[test]
fn static_font_gets_synthetic_bold_and_italic() {
    let l = lib();
    let r = l.resolve(MONO, true, true).unwrap();
    assert!(r.synthetic_bold && r.synthetic_italic);
    let reg = layout_text(&spec("I", MONO), &l).unwrap();
    let mut s = spec("I", MONO);
    s.bold = true;
    let bold = layout_text(&s, &l).unwrap();
    assert!(bold.bounds.width() > reg.bounds.width() + 0.1);
    assert!(bold.bounds.height() > reg.bounds.height());
    let mut s = spec("I", MONO);
    s.italic = true;
    let it = layout_text(&s, &l).unwrap();
    // The top of the glyph leans right.
    assert!(it.bounds.x1 > reg.bounds.x1 + 1.0);
    assert!((it.bounds.height() - reg.bounds.height()).abs() < 1e-6);
}

#[test]
fn bidi_reorders_right_to_left_paragraphs() {
    let l = lib();
    // Hebrew letters in a left-to-right line: the Hebrew run is drawn reversed, Latin stays put.
    let mut s = spec("a\u{05D0}\u{05D1}b", MONO);
    let lay = layout_text(&s, &l).unwrap();
    let order: Vec<&str> = lay.glyphs.iter().map(|g| g.text.as_str()).collect();
    assert_eq!(order, vec!["a", "\u{05D1}", "\u{05D0}", "b"]);
    assert!(lay.glyphs[1].rtl && !lay.glyphs[0].rtl);
    // Pure Latin is not reordered even in a right-to-left paragraph.
    s.text = "ab".into();
    s.direction = Direction::Rtl;
    let lay = layout_text(&s, &l).unwrap();
    let order: Vec<&str> = lay.glyphs.iter().map(|g| g.text.as_str()).collect();
    assert_eq!(order, vec!["a", "b"]);
    // A right-to-left paragraph puts the Hebrew run before (left of) nothing, but flips run order.
    s.text = "a \u{05D0}".into();
    let lay = layout_text(&s, &l).unwrap();
    let order: Vec<&str> = lay.glyphs.iter().map(|g| g.text.as_str()).collect();
    assert_eq!(order, vec!["\u{05D0}", " ", "a"]);
}

#[test]
fn missing_glyphs_are_flagged_without_system_fonts() {
    let l = lib();
    let lay = layout_text(&spec("\u{05D0}", MONO), &l).unwrap(); // Hebrew alef, not in Plex Mono
    assert!(lay.glyphs[0].missing);
}

#[test]
fn complex_scripts_and_cjk_use_system_fonts_when_present() {
    // Environment dependent: only asserts when the machine has fonts for the script.
    let l = FontLibrary::with_system_fonts();
    if l.fallback_for('\u{0633}').is_some() {
        let lay = layout_text(&spec("\u{0633}\u{0644}\u{0627}\u{0645}", "Arial"), &l).unwrap();
        assert!(!lay.glyphs.is_empty());
        assert!(lay.glyphs.iter().all(|g| !g.missing));
        assert!(lay.glyphs.iter().all(|g| g.rtl));
        // Shaping joins letters: 4 characters may become fewer or equal glyphs, never more.
        assert!(lay.glyphs.len() <= 4);
    }
    if l.fallback_for('\u{6F22}').is_some() {
        let lay = layout_text(&spec("\u{6F22}\u{5B57}", "Arial"), &l).unwrap();
        assert_eq!(lay.glyphs.len(), 2);
        assert!(lay.glyphs.iter().all(|g| !g.missing && !g.path.elements().is_empty()));
    }
}

#[test]
fn weld_unions_overlapping_glyphs_and_keeps_counters() {
    let l = lib();
    let mut s = spec("II", MONO);
    s.letter_spacing = -3.0; // heavy overlap
    let lay = layout_text(&s, &l).unwrap();
    assert_eq!(lay.outlines(false).len(), 2);
    let w = lay.outlines(true);
    assert_eq!(w.len(), 1, "overlapping I's become one shape");
    let area_sum: f64 = lay.outlines(false).iter().map(|p| p.area().abs()).sum();
    assert!(w[0].area().abs() < area_sum);

    let o = layout_text(&spec("O", MONO), &l).unwrap();
    let ow = o.outlines(true);
    assert_eq!(ow.len(), 1);
    assert_eq!(moves(&ow), 2, "outer ring plus the counter");
    // Separate letters stay separate shapes.
    let ab = layout_text(&spec("II", MONO), &l).unwrap();
    assert_eq!(ab.outlines(true).len(), ab.outlines(false).len());
}

#[test]
fn weld_accepts_a_caller_union() {
    let l = lib();
    let lay = layout_text(&spec("ab", MONO), &l).unwrap();
    let out = lay.outlines_with(&|ps| vec![ps[0].clone()]);
    assert_eq!(out.len(), 1);
}

#[test]
fn backup_outline_survives_a_missing_font() {
    let l = lib();
    let s = spec("Backup", MONO);
    let (lay, src) = layout_with_backup(&s, &l, None).unwrap();
    assert_eq!(src, LayoutSource::Font);
    let backup = lay.backup(&s);
    let json = serde_json::to_string(&backup).unwrap();
    let backup: BackupOutline = serde_json::from_str(&json).unwrap();

    // A machine that has other fonts but not this one.
    let mut other = FontLibrary::new();
    other.load_font_file(font_dir().join("IBMPlexSans-Variable.ttf")).unwrap();
    assert!(font_missing(&s, &other));
    let (lay2, src2) = layout_with_backup(&s, &other, Some(&backup)).unwrap();
    assert_eq!(src2, LayoutSource::Backup);
    assert_eq!(lay2.glyphs.len(), lay.glyphs.len());
    assert!((lay2.bounds.width() - lay.bounds.width()).abs() < 1e-6);
    assert!((lay2.bounds.height() - lay.bounds.height()).abs() < 1e-6);

    // Editing the text invalidates the backup: use the fallback font instead.
    let mut edited = s.clone();
    edited.text = "Edited".into();
    let (_, src3) = layout_with_backup(&edited, &other, Some(&backup)).unwrap();
    assert_eq!(src3, LayoutSource::Fallback);
    // No backup at all also falls back.
    let (lay4, src4) = layout_with_backup(&s, &other, None).unwrap();
    assert_eq!(src4, LayoutSource::Fallback);
    assert_eq!(lay4.family_used, SANS);
}

#[test]
fn empty_library_is_an_error_and_empty_text_is_fine() {
    let empty = FontLibrary::new();
    assert!(layout_text(&spec("x", "Nope"), &empty).is_err());
    let l = lib();
    let lay = layout_text(&spec("", MONO), &l).unwrap();
    assert!(lay.glyphs.is_empty());
    assert!(lay.outlines(true).is_empty());
}

#[test]
fn spec_round_trips_through_serde() {
    let mut s = spec("Hi", MONO);
    s.bold = true;
    s.h_align = HAlign::Center;
    let j = serde_json::to_string(&s).unwrap();
    let back: TextSpec = serde_json::from_str(&j).unwrap();
    assert_eq!(s, back);
    // Missing fields use defaults.
    let d: TextSpec = serde_json::from_str("{\"text\":\"x\"}").unwrap();
    assert_eq!(d.size, 10.0);
}

#[test]
fn glyph_count_counts_the_characters_a_face_can_draw() {
    let l = lib();
    let id = l.resolve(SANS, false, false).unwrap().id;
    assert_eq!(l.glyph_count(id, &['A', 'z', '\u{e5}']), 3);
    // Cyrillic and Greek are in Plex Sans; CJK and Arabic are not.
    assert_eq!(l.glyph_count(id, &['\u{0416}', '\u{03A9}', '\u{65E5}', '\u{0633}']), 2);
    assert_eq!(l.glyph_count(id, &[]), 0);
    // The bytes of a face can be shared as they are.
    let data = l.face_data(id).unwrap();
    let bytes: &[u8] = (*data).as_ref();
    assert_eq!(bytes.len(), data.data.len());
}
