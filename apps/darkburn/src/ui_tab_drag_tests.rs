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

#[test]
fn a_docked_window_tab_dragged_onto_a_floating_group_joins_it() {
    use darkburn_ui::ui_kit::window_host::{self, Placement};
    let mut g = two_groups();
    g.app().settings_open = true;
    g.frames(2);
    window_host::update(&g.ctx(), "settings", |p| p.dock("Bottom"));
    g.frames(3);
    let id = g.app().prefs.float_panel("laser", [300.0, 200.0], [360.0, 400.0]).unwrap();
    g.frames(3);
    let float = g.rect_of(&format!("panel:float:{id}")).expect("the floating group is drawn");
    let settings = g.rect_of("panel:tab:window:settings").unwrap_or_else(|| panic!("the settings tab: {:?}", g.rects("panel:tab:")));
    g.drag(settings.center(), float.center());
    g.frames(3);
    let key = Place::Floating(id).key();
    let placement = |g: &Gui| g.app().prefs.windows.get("settings").map(|p| p.placement.clone()).unwrap_or_default();
    assert_eq!(placement(&g), Placement::Docked(key.clone()), "it docks in the floating group");
    let tabs: Vec<String> = window_host::docked_tabs(&g.ctx(), &key).into_iter().map(|t| t.key).collect();
    assert_eq!(tabs, ["settings"]);
    let wtab = g.rect_of("panel:tab:window:settings").expect("its tab is drawn");
    assert!(float.contains(wtab.center()), "its tab is in the group's bar: {wtab:?} {float:?}");
    // The panel leaves; the window keeps the group alive.
    let right = g.rect_of("panel:area:RightTop").expect("the right tab group");
    g.drag(tab(&g, "laser").center(), right.center());
    g.frames(2);
    assert!(g.app().prefs.floating.contains_key(&id), "the window keeps the group");
    // Closing the group closes the window, which opens again in the right dock.
    let close = g.rect_of(&format!("panel:float_close:{id}")).expect("its close button");
    g.click_pos(close.center());
    g.frames(3);
    assert!(g.app().prefs.floating.is_empty());
    assert_eq!(placement(&g), Placement::Docked("RightTop".into()));
}

#[test]
fn a_tab_dropped_past_the_last_shown_tab_of_a_full_bar_goes_to_the_end() {
    // The default layout: every right panel in one bar, some behind the chevron.
    let mut g = Gui::with_prefs(1600.0, 1000.0, 1.0, Prefs::default());
    g.frames(3);
    let right = Place::Docked(Area::RightTop);
    assert!(g.rect_of("panel:overflow:RightTop").is_some(), "the bar overflows: {:?}", g.rects("panel:tab:"));
    let first = order(&g, right)[0];
    let shown: Vec<Rect> = order(&g, right).iter().filter_map(|id| g.rect_of(&format!("panel:tab:{id}"))).collect();
    let last = shown.iter().map(|r| r.right()).fold(f32::MIN, f32::max);
    let to = Pos2::new(last + 4.0, tab(&g, first).center().y);
    g.drag(tab(&g, first).center(), to);
    assert_eq!(order(&g, right).last().copied(), Some(first), "{:?}", order(&g, right));
}

#[test]
fn a_full_floating_group_puts_its_extra_tabs_behind_a_chevron() {
    let mut g = two_groups();
    let id = g.app().prefs.float_panel("laser", [300.0, 200.0], [200.0, 300.0]).unwrap();
    for p in ["library", "cuts", "console", "camera"] {
        g.app().prefs.move_panel(p, Place::Floating(id), None);
    }
    g.frames(3);
    let float = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    let more = g.rect_of(&format!("panel:float_overflow:{id}")).unwrap_or_else(|| panic!("the chevron: {:?}", g.rects("panel:")));
    assert!(float.contains(more.center()));
    for r in g.rects("panel:tab:").into_iter().map(|(_, r)| r).filter(|r| float.intersects(*r)) {
        assert!(r.right() <= more.left() + 0.5 && r.width() > 1.0, "a shown tab is whole and left of the chevron: {r:?} {more:?}");
    }
}

#[test]
fn a_floating_group_stored_off_screen_is_drawn_on_it_and_stays_there_when_moved() {
    let mut g = two_groups();
    let id = g.app().prefs.float_panel("laser", [3000.0, -500.0], [360.0, 400.0]).unwrap();
    g.frames(2);
    let screen = g.screen();
    let float = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    assert!(screen.contains_rect(float), "{float:?} on {screen:?}");
    // Its header dragged far up and left: it stops at the edge.
    let free = tab(&g, "laser").right_center() + vec2(60.0, 0.0);
    g.drag(free, Pos2::new(free.x - 2000.0, 1.0));
    let moved = g.rect_of(&format!("panel:float:{id}")).expect("drawn");
    assert!(screen.contains_rect(moved), "{moved:?} on {screen:?}");
    let f = &g.app().prefs.floating[&id];
    assert!(f.pos[0] >= screen.left() && f.pos[1] >= screen.top(), "the stored place is on screen too: {:?}", f.pos);
}
