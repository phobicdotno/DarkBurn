mod common;
use common::*;
use darkburn_text::fontprefs::size_of;
use darkburn_text::*;

#[test]
fn recent_list_is_mru_unique_and_bounded() {
    let mut p = FontPrefs { max_recent: 3, ..Default::default() };
    for f in ["A", "B", "C", "A", "D"] {
        p.touch(f);
    }
    assert_eq!(p.recent, vec!["D", "A", "C"]);
    p.touch("");
    p.touch("c");
    assert_eq!(p.recent, vec!["c", "D", "A"]);
}

#[test]
fn favourites_toggle() {
    let mut p = FontPrefs::default();
    assert!(p.toggle_favourite("Arial"));
    assert!(p.is_favourite("arial"));
    assert!(!p.toggle_favourite("ARIAL"));
    assert!(p.favourites.is_empty());
}

#[test]
fn listing_sections_and_filter() {
    let l = lib();
    let installed = list_families(&l);
    let mut p = FontPrefs::default();
    p.toggle_favourite(MONO);
    p.toggle_favourite("Gone Font");
    p.touch(SANS);
    p.touch("Gone Font");
    let ls = p.listing(&installed, "", false);
    assert_eq!(ls.favourites, vec![MONO]);
    assert_eq!(ls.recent, vec![SANS]);
    assert_eq!(ls.all, installed);
    let kept = p.listing(&installed, "", true);
    assert_eq!(kept.favourites.len(), 2);
    let f = p.listing(&installed, "mono", false);
    assert_eq!(f.all, vec![MONO]);
    assert!(f.recent.is_empty());
}

#[test]
fn default_font_for_new_text() {
    let l = lib();
    let mut p = FontPrefs { default_family: MONO.into(), ..Default::default() };
    assert_eq!(p.family_for_new_text(&l), MONO);
    p.default_family = "Not Installed".into();
    assert_eq!(p.family_for_new_text(&l), SANS);
    let j = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<FontPrefs>(&j).unwrap(), p);
}

#[test]
fn preview_fits_the_box_and_is_centred() {
    let l = lib();
    let out = preview_outline(&l, MONO, "Abc 123", 40.0, 8.0).unwrap();
    assert!(!out.is_empty());
    let (w, h) = size_of(&out);
    assert!(w <= 40.0 + 1e-6 && h <= 8.0 + 1e-6);
    assert!((w - 40.0).abs() < 1e-6 || (h - 8.0).abs() < 1e-6, "one dimension fills the box");
    let b = darkburn_text::layout::outlines_bbox(&out);
    assert!(b.center().x.abs() < 1e-6 && b.center().y.abs() < 1e-6);
    // Missing family previews with the default font instead of failing.
    assert!(!preview_outline(&l, "Nope", "x", 10.0, 10.0).unwrap().is_empty());
    assert!(preview_outline(&l, MONO, "", 10.0, 10.0).unwrap().is_empty());
}
