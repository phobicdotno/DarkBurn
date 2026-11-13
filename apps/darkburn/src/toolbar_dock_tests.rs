//! Toolbars that move [TB-25], in the real window: docked at each edge with their buttons in a
//! row or a column, floating, dragged by their background or grip, moved from their right-click
//! menu, kept over a restart and per device, and put back by Reset to Default Layout.

use egui::{Pos2, Rect, pos2, vec2};
use serde_json::json;

use crate::devices::{Device, DeviceType};
use crate::machine_ui;
use crate::prefs::Prefs;
use crate::shell::DarkBurn;
use crate::shell::TOOLS_WIDTH;
use crate::toolbar_dock::{Dock, dock_of, home_edge};
use crate::ui_harness::Gui;

fn bar(g: &Gui, id: &str) -> Rect {
    g.rect_of(&format!("bar:{id}")).unwrap_or_else(|| panic!("toolbar {id} is shown away from its usual place"))
}

/// The buttons of a moved toolbar (the cut palette's layer buttons for the palette).
fn buttons(g: &Gui, id: &str) -> Vec<Rect> {
    let prefix = if id == "palette" { "palette:".to_string() } else { format!("bar:{id}:") };
    g.rects(&prefix).into_iter().filter(|(n, _)| !n.ends_with(":grip")).map(|(_, r)| r).collect()
}

/// How many columns and rows the buttons stand in.
fn columns_and_rows(bs: &[Rect]) -> (usize, usize) {
    let count = |f: &dyn Fn(&Rect) -> f32| {
        let mut v: Vec<i32> = bs.iter().map(|r| f(r).round() as i32).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    (count(&|r| r.center().x), count(&|r| r.center().y))
}

fn place(g: &mut Gui, id: &str, to: &str) {
    g.exec("window.toolbar_place", json!({ "toolbar": id, "place": to }));
    g.frames(3);
}

/// Every button inside the screen and inside its toolbar, and none on top of another.
fn tidy(g: &Gui, id: &str) {
    let screen = g.screen();
    let strip = bar(g, id);
    let bs = buttons(g, id);
    assert!(bs.len() >= 3, "{id}: {bs:?}");
    for (i, a) in bs.iter().enumerate() {
        assert!(screen.contains_rect(*a) && strip.contains_rect(*a), "{id}: button {a:?} is off the screen or off {strip:?}");
        for b in &bs[i + 1..] {
            assert!(!a.shrink(0.5).intersects(*b), "{id}: buttons {a:?} and {b:?} overlap");
        }
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 2.0
}

#[test]
fn every_toolbar_docks_at_each_edge_in_a_row_or_a_column() {
    let mut g = Gui::standard();
    let screen = g.screen();
    for id in ["main", "tools", "palette", "modes"] {
        for to in ["left", "right", "top", "bottom", "floating"] {
            if home_edge(id) == Dock::parse(to) {
                continue; // its usual place, drawn as always (the golden picture)
            }
            place(&mut g, id, to);
            let r = bar(&g, id);
            match to {
                // Inside the tools column, which stays at the very edge.
                "left" => assert!(r.left() <= screen.left() + TOOLS_WIDTH + 1.0, "{id} at the left: {r:?}"),
                "right" => assert!(near(r.right(), screen.right()), "{id} at the right: {r:?}"),
                "top" => assert!(r.top() > 120.0 && r.top() < 200.0, "{id} at the top, under the ribbon: {r:?}"),
                "bottom" => assert!(r.bottom() > screen.bottom() - 60.0, "{id} at the bottom: {r:?}"),
                _ => assert!(r.left() > screen.left() + 50.0 && r.top() > 150.0, "{id} floats: {r:?}"),
            }
            let (cols, rows) = columns_and_rows(&buttons(&g, id));
            if Dock::parse(to).unwrap().vertical() {
                assert!(r.height() > r.width() && rows > cols, "{id} at the {to}: a column ({cols} x {rows}) {r:?}");
            } else {
                assert!(r.width() > r.height() && cols > rows, "{id} at the {to}: a row ({cols} x {rows}) {r:?}");
            }
            tidy(&g, id);
        }
        place(&mut g, id, "home");
        assert!(g.rect_of(&format!("bar:{id}")).is_none(), "{id} back in its usual place");
    }
    // A ribbon toolbar out of the ribbon is not in it twice.
    place(&mut g, "main", "right");
    assert!(g.rect_of("ribbon:file.new").is_none() && g.rect_of("bar:main:file.new").is_some());
    // The buttons work where they are.
    place(&mut g, "tools", "right");
    assert!(g.rect_of("tool:tools.rect").unwrap().left() > screen.right() - 120.0, "the tools are at the right");
    g.click_id("bar:tools:tools.rect");
    assert_eq!(g.app().tool.command(), "tools.rect");
    place(&mut g, "palette", "top");
    g.click_id("palette:07");
    assert_eq!(g.app().editor.current_layer, 7, "the palette's buttons work at the top");
    assert!(g.clashes().is_empty(), "{:?}", g.clashes());
    g.screenshot("toolbars-at-the-edges");
}

#[test]
fn wide_controls_move_with_their_toolbar() {
    let mut g = Gui::standard();
    g.sample_design();
    for id in ["numeric", "text", "docking"] {
        for to in ["left", "floating", "top"] {
            place(&mut g, id, to);
            let r = bar(&g, id);
            assert!(g.screen().intersects(r), "{id} {to}: {r:?}");
        }
        place(&mut g, id, "home");
    }
    // The numeric edits at the right still edit the selection.
    place(&mut g, "numeric", "right");
    g.frames(3);
    let r = bar(&g, "numeric");
    assert!(near(r.right(), g.screen().right()), "{r:?}");
    assert!(g.clashes().is_empty(), "{:?}", g.clashes());
    g.screenshot("toolbar-numeric-right");
}

#[test]
fn dragging_a_toolbar_docks_it_floats_it_and_puts_it_back() {
    let mut g = Gui::standard();
    let screen = g.screen();
    // The cut palette by its background (right of its buttons) to the left edge.
    let last = g.rect_of("palette:T2").expect("the palette");
    let from = pos2(screen.right() - 200.0, last.center().y);
    assert!(from.x > last.right() + 20.0);
    g.drag(from, pos2(20.0, 500.0));
    assert_eq!(dock_of(&g.app().prefs, "palette"), Dock::Left);
    g.frames(2);
    let (a, b) = (g.rect_of("palette:00").unwrap(), g.rect_of("palette:01").unwrap());
    assert!(a.left() < 60.0 && near(a.center().x, b.center().x) && b.top() > a.top(), "a column at the left: {a:?} {b:?}");

    // The File group of the ribbon by its caption into the middle: Main floats there.
    let open = g.rect_of("ribbon:file.open").expect("the File group");
    g.drag(pos2(open.center().x, open.bottom() + 14.0), pos2(800.0, 500.0));
    assert_eq!(dock_of(&g.app().prefs, "main"), Dock::Floating);
    g.frames(2);
    let r = bar(&g, "main");
    assert!(r.min.distance(pos2(790.0, 480.0)) < 4.0, "the grip under the pointer: {r:?}");
    assert!(g.rect_of("ribbon:file.open").is_none());
    // Floated again it stays where it was let go, also over a restart.
    let grip = g.rect_of("bar:main:grip").unwrap();
    g.drag(grip.center(), grip.center() + vec2(200.0, 100.0));
    let moved = bar(&g, "main");
    assert!(moved.min.distance(r.min + vec2(200.0, 100.0)) < 4.0, "{r:?} -> {moved:?}");

    // By its grip onto the ribbon: back in it.
    let grip = g.rect_of("bar:main:grip").unwrap();
    let ribbon = g.rect_of("ribbon:tab:home").unwrap();
    g.drag(grip.center(), pos2(900.0, ribbon.bottom() + 40.0));
    assert_eq!(dock_of(&g.app().prefs, "main"), Dock::Home);
    g.frames(2);
    assert!(g.rect_of("ribbon:file.open").is_some() && g.rect_of("bar:main").is_none());

    // The tools column by its background to the right edge, and back to the left.
    let last_tool = g.rect_of("tool:tools.qr").unwrap();
    g.drag(pos2(last_tool.center().x, last_tool.bottom() + 60.0), pos2(screen.right() - 10.0, 500.0));
    assert_eq!(dock_of(&g.app().prefs, "tools"), Dock::Right);
    g.frames(2);
    let grip = g.rect_of("bar:tools:grip").unwrap();
    g.drag(grip.center(), pos2(10.0, 600.0));
    assert_eq!(dock_of(&g.app().prefs, "tools"), Dock::Home, "the tools' own edge is their usual place");
    // A press that does not move leaves a toolbar where it is.
    let grip = g.rect_of("bar:palette:grip").unwrap();
    g.click_pos(grip.center());
    assert_eq!(dock_of(&g.app().prefs, "palette"), Dock::Left);
    assert!(g.clashes().is_empty(), "{:?}", g.clashes());
}

#[test]
fn the_right_click_menu_docks_floats_and_hides() {
    let mut g = Gui::standard();
    let screen = g.screen();
    let last = g.rect_of("palette:T2").unwrap();
    g.right_click_pos(pos2(screen.right() - 200.0, last.center().y));
    assert!(g.rect_of("menu:bar_bottom").is_some() && g.rect_of("menu:bar_home").is_none(), "the palette's home is the bottom");
    g.click_id("menu:bar_right");
    assert_eq!(dock_of(&g.app().prefs, "palette"), Dock::Right);
    g.frames(2);
    // A ribbon group's menu.
    let open = g.rect_of("ribbon:file.open").unwrap();
    g.right_click_pos(pos2(open.center().x, open.bottom() + 14.0));
    g.click_id("menu:bar_float");
    assert_eq!(dock_of(&g.app().prefs, "main"), Dock::Floating);
    g.frames(2);
    let grip = g.rect_of("bar:main:grip").unwrap();
    g.right_click_pos(grip.center());
    g.click_id("menu:bar_left");
    assert_eq!(dock_of(&g.app().prefs, "main"), Dock::Left);
    g.frames(2);
    let grip = g.rect_of("bar:main:grip").unwrap();
    g.right_click_pos(grip.center());
    g.click_id("menu:bar_home");
    assert_eq!(dock_of(&g.app().prefs, "main"), Dock::Home);
    g.frames(2);
    assert!(g.rect_of("ribbon:file.open").is_some());
    // Hide from the menu; the Window menu shows it again where it was.
    let grip = g.rect_of("bar:palette:grip").unwrap();
    g.right_click_pos(grip.center());
    g.click_id("menu:bar_hide");
    assert!(!g.app().prefs.toolbar("palette") && g.rect_of("palette:00").is_none());
    g.run_command("toolbar.palette");
    assert!(g.app().prefs.toolbar("palette"));
    assert!(near(bar(&g, "palette").right(), screen.right()));
    assert!(g.clashes().is_empty(), "{:?}", g.clashes());
}

#[test]
fn toolbar_places_survive_a_restart_and_reset_puts_them_back() {
    let mut g = Gui::standard();
    place(&mut g, "main", "left");
    place(&mut g, "tools", "top");
    place(&mut g, "palette", "right");
    g.exec("window.toolbar_place", json!({ "toolbar": "modes", "place": "floating", "x": 600, "y": 420 }));
    g.frames(3);
    g.screenshot("toolbars-moved");
    let stored = g.app().prefs.to_json();
    let mut g = Gui::with_prefs(1600.0, 1000.0, 1.0, Prefs::from_json(&stored));
    g.frames(3);
    let screen = g.screen();
    assert!(near(bar(&g, "main").left(), screen.left()));
    assert!(bar(&g, "tools").top() < 200.0 && bar(&g, "tools").width() > bar(&g, "tools").height());
    assert!(near(bar(&g, "palette").right(), screen.right()));
    assert_eq!(bar(&g, "modes").min, pos2(600.0, 420.0));
    assert!(g.rect_of("ribbon:file.open").is_none());
    g.run_command("window.reset_layout");
    g.frames(2);
    assert!(g.app().prefs.toolbar_places.is_empty());
    assert!(g.rects("bar:").is_empty(), "{:?}", g.rects("bar:"));
    assert!(g.rect_of("ribbon:file.open").is_some());
    assert!(g.rect_of("tool:tools.select").unwrap().left() < 40.0);
    let p = g.rect_of("palette:00").unwrap();
    assert!(p.bottom() > screen.bottom() - 60.0, "the palette back at the bottom: {p:?}");
    g.screenshot("toolbars-reset");
}

#[test]
fn each_device_keeps_its_toolbar_places() {
    let mut a = DarkBurn::new(Prefs::default());
    machine_ui::adopt_device_state(&mut a);
    let first = a.machine.devices.active.unwrap();
    crate::toolbar_dock::set_place(&mut a.prefs, "main", Dock::Left, None);
    let mut d = Device::new(DeviceType::Grbl);
    d.name = "Second".into();
    let second = a.machine.devices.add(d);
    a.machine.select_device(second);
    machine_ui::adopt_device_state(&mut a);
    assert_eq!(dock_of(&a.prefs, "main"), Dock::Left, "a new device takes over the current places");
    crate::toolbar_dock::set_place(&mut a.prefs, "palette", Dock::Floating, Some(Pos2::new(40.0, 50.0)));
    crate::toolbar_dock::set_place(&mut a.prefs, "main", Dock::Home, None);
    a.machine.select_device(first);
    machine_ui::adopt_device_state(&mut a);
    assert_eq!((dock_of(&a.prefs, "main"), dock_of(&a.prefs, "palette")), (Dock::Left, Dock::Home), "the first device's places");
    a.machine.select_device(second);
    machine_ui::adopt_device_state(&mut a);
    assert_eq!((dock_of(&a.prefs, "main"), dock_of(&a.prefs, "palette")), (Dock::Home, Dock::Floating));
    // A device stored before toolbars moved keeps the current places.
    let mut old = Device::new(DeviceType::Grbl);
    old.state.panel_layout = serde_json::to_string(&a.prefs.panels).ok();
    let old = a.machine.devices.add(old);
    a.machine.select_device(old);
    machine_ui::adopt_device_state(&mut a);
    assert_eq!(dock_of(&a.prefs, "palette"), Dock::Floating);
}
