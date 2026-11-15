//! Panel tabs dragged like window tabs: onto another tab group, along their own bar, out into a
//! floating group of their own and back, and the floating group's own header, corner and close.

use egui::{Pos2, Rect, vec2};

use crate::panels::{Area, Place};
use crate::prefs::Prefs;
use crate::ui_harness::Gui;

/// Laser and Library in the bottom dock, so there are two docked tab groups.
fn two_groups() -> Gui {
    let mut prefs = Prefs::default();
    prefs.move_panel("laser", Place::Docked(Area::Bottom), None);
    prefs.move_panel("library", Place::Docked(Area::Bottom), None);
    let mut g = Gui::with_prefs(1600.0, 1000.0, 1.0, prefs);
    g.frames(3);
    g
}

fn tab(g: &Gui, id: &str) -> Rect {
    g.rect_of(&format!("panel:tab:{id}")).unwrap_or_else(|| panic!("the {id} tab: {:?}", g.rects("panel:tab:")))
}

fn canvas_spot(g: &Gui) -> Pos2 {
    let s = g.screen();
    Pos2::new(s.center().x - 200.0, s.center().y - 100.0)
}

fn order(g: &Gui, place: Place) -> Vec<&'static str> {
    g.app().prefs.visible_in(place).iter().map(|p| p.id).collect()
}

#[test]
fn a_panel_tab_dragged_onto_another_group_joins_it() {
    let mut g = two_groups();
    assert_eq!(g.app().prefs.place("laser"), Place::Docked(Area::Bottom));
    let right = g.rect_of("panel:area:RightTop").expect("the right tab group");
    g.drag(tab(&g, "laser").center(), right.center());
    assert_eq!(g.app().prefs.place("laser"), Place::Docked(Area::RightTop));
    assert_eq!(g.app().prefs.front(Place::Docked(Area::RightTop)).as_deref(), Some("laser"), "it comes to the front");
    assert_eq!(g.app().prefs.place("library"), Place::Docked(Area::Bottom), "the other tab stays");
}

#[test]
fn a_panel_tab_dragged_along_its_bar_reorders_and_the_order_is_kept() {
    let mut g = two_groups();
    let bottom = Place::Docked(Area::Bottom);
    assert_eq!(order(&g, bottom), vec!["laser", "library"]);
    let to = tab(&g, "library").right_center() + vec2(6.0, 0.0);
    g.drag(tab(&g, "laser").center(), to);
    assert_eq!(order(&g, bottom), vec!["library", "laser"]);
    let saved = g.app().prefs.tab_order.get(&bottom.key()).cloned().unwrap_or_default();
    let at = |id: &str| saved.iter().position(|p| p == id);
    assert!(at("library").is_some() && at("library") < at("laser"), "the stored order: {saved:?}");
}

#[test]
fn a_panel_tab_dragged_out_floats_and_dragged_back_docks() {
    let mut g = two_groups();
    let spot = canvas_spot(&g);
    g.drag(tab(&g, "laser").center(), spot);
    let place = g.app().prefs.place("laser");
    let Place::Floating(id) = place else { panic!("laser floats: {place:?}") };
    let float = g.rect_of(&format!("panel:float:{id}")).expect("the floating group is drawn");
    assert!(float.contains(spot), "it floats where it was let go: {float:?} {spot:?}");
    // Another tab joins the floating group.
    g.drag(tab(&g, "library").center(), float.center());
    assert_eq!(g.app().prefs.place("library"), Place::Floating(id));
    // Both back to the right; the group is gone with its last tab.
    let right = g.rect_of("panel:area:RightTop").expect("the right tab group");
    g.drag(tab(&g, "laser").center(), right.center());
    assert_eq!(g.app().prefs.floating.len(), 1, "one tab is still there");
    g.drag(tab(&g, "library").center(), right.center());
    assert_eq!(g.app().prefs.place("laser"), Place::Docked(Area::RightTop));
    assert_eq!(g.app().prefs.place("library"), Place::Docked(Area::RightTop));
    assert!(g.app().prefs.floating.is_empty());
    assert!(g.rects("panel:float:").is_empty());
}

#[test]
fn a_floating_group_moves_resizes_and_closes() {
    let mut g = two_groups();
    let id = g.app().prefs.float_panel("laser", [300.0, 200.0], [360.0, 400.0]).unwrap();
    g.frames(2);
    let before = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    // The header's free space, right of the only tab, moves it.
    let free = tab(&g, "laser").right_center() + vec2(60.0, 0.0);
    g.drag(free, free + vec2(100.0, 50.0));
    let moved = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    assert!((moved.min - before.min - vec2(100.0, 50.0)).length() < 2.0, "{before:?} -> {moved:?}");
    let corner = g.rect_of(&format!("panel:float_resize:{id}")).expect("the corner");
    g.drag(corner.center(), corner.center() + vec2(40.0, 30.0));
    let sized = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    assert!((sized.size() - moved.size() - vec2(40.0, 30.0)).length() < 2.0, "{moved:?} -> {sized:?}");
    let close = g.rect_of(&format!("panel:float_close:{id}")).expect("its close button");
    g.click_pos(close.center());
    g.frames(2);
    assert!(g.app().prefs.floating.is_empty());
    assert!(!g.app().prefs.panel("laser").visible, "closing hides its panels");
    assert!(g.rects("panel:float:").is_empty());
}
