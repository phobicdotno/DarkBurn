//! Second bug-hunt pass over the interface: tools under dialogs, panels and dialogs.
//! See docs/UI-TESTING.md.
use egui::{Key, Modifiers};

use crate::tools::Tool;
use crate::ui_harness::Gui;

/// The node tool reads its keys itself: over an open dialog they belong to the dialog, like the
/// select tool's keys.
#[test]
fn node_tool_keys_over_an_open_dialog_do_not_edit_the_drawing() {
    for window in ["help.about", "edit.settings", "tools.offset"] {
        let mut g = Gui::standard();
        g.sample_design();
        g.app().set_tool(Tool::Nodes);
        g.frames(2);
        let before = g.app().editor.doc.clone();
        g.run_command(window);
        assert!(g.dialog_open(), "{window} opened a window");
        for key in [Key::Delete, Key::Backspace, Key::ArrowRight, Key::I, Key::S, Key::L, Key::C, Key::B, Key::T, Key::E] {
            g.key(key, Modifiers::NONE);
            assert!(g.app().editor.doc == before, "{window}: {key:?} changed the drawing behind the window");
        }
        g.key(Key::D, Modifiers::COMMAND);
        assert!(g.app().editor.doc == before, "{window}: Ctrl+D changed the drawing behind the window");
        g.close_dialogs();
        g.key(Key::Delete, Modifiers::NONE);
        assert_eq!(g.app().editor.doc.len(), 0, "{window}: Delete works again once the window is closed");
    }
}

/// Typing on the canvas with the text tool ends when a dialog opens over it: what is typed then
/// goes to the dialog, not into the text behind it.
#[test]
fn text_typed_over_an_open_dialog_does_not_go_into_the_text_behind_it() {
    for window in ["help.about", "edit.settings", "arrange.move_selection"] {
        let mut g = Gui::standard();
        g.app().set_tool(Tool::Text);
        g.frames(2);
        g.click_mm(100.0, 100.0);
        g.type_text("ab");
        let text = |g: &Gui| {
            g.app().editor.doc.iter().find_map(|s| match &s.kind {
                darkburn_core::ShapeKind::Text(t) => Some(t.text.clone()),
                _ => None,
            })
        };
        assert_eq!(text(&g).as_deref(), Some("ab"));
        g.run_command(window);
        assert!(g.dialog_open(), "{window} opened a window");
        g.type_text("c");
        g.key(Key::Backspace, Modifiers::NONE);
        g.key(Key::Enter, Modifiers::NONE);
        assert_eq!(text(&g).as_deref(), Some("ab"), "{window}: keys over the window edited the text behind it");
        g.close_dialogs();
        assert_eq!(text(&g).as_deref(), Some("ab"));
        // With the window gone the typing goes on.
        g.type_text("c");
        assert_eq!(text(&g).as_deref(), Some("abc"), "{window}: typing did not go on after the window closed");
    }
}

fn show_panel(g: &mut Gui, panel: &str) {
    if !g.app().prefs.panel(panel).visible {
        g.run_command(&format!("panel.{panel}"));
    }
    let area = g.app().prefs.panel(panel).area;
    g.app().prefs.active_tab.insert(area, panel.to_string());
    g.frames(3);
}

/// Close on an art library with unsaved changes asks first: save, drop the changes, or keep it
/// open. A library without changes closes at once.
#[test]
fn closing_an_art_library_with_unsaved_changes_asks_first() {
    use serde_json::json;
    let mut g = Gui::standard();
    g.sample_design();
    show_panel(&mut g, "art_library");
    let open = |g: &Gui| g.app().extras.art.libs.len();
    g.exec("art.new", json!({}));
    g.click_label("Close");
    assert_eq!(open(&g), 0, "a library without changes closes at once");

    g.exec("art.new", json!({}));
    g.exec("art.add_selection", json!({ "library": 0, "name": "Box" }));
    assert!(g.app().extras.art.libs[0].dirty);
    g.click_label("Close");
    assert_eq!(open(&g), 1, "the library closed with unsaved changes, without a question");
    g.click_label("Keep open");
    assert_eq!(open(&g), 1);
    assert!(!g.has_text("Keep open"), "the question is gone");
    // Save and close with no file picked: it stays open with its changes.
    g.click_label("Close");
    g.click_label("Save and close");
    assert!(open(&g) == 1 && g.app().extras.art.libs[0].dirty, "a cancelled save closed the library");
    let file = g.temp("kept.dbart");
    g.answer_dialog(&file);
    g.click_label("Save and close");
    assert_eq!(open(&g), 0);
    let saved = crate::art_library::Open::load(&file).unwrap();
    assert_eq!(saved.lib.folders[0].items[0].name, "Box", "the changes are in the file");

    g.exec("art.new", json!({}));
    g.exec("art.add_selection", json!({ "library": 0, "name": "Box" }));
    g.click_label("Close");
    g.click_label("Close without saving");
    assert_eq!(open(&g), 0);
}

/// A question about an item ("is already in this folder") goes away with its library; it used to
/// stay and its buttons then reached for a library that was no longer there.
#[test]
fn closing_an_art_library_drops_the_question_about_its_item() {
    use crate::art_library::Item;
    use serde_json::json;
    let mut g = Gui::standard();
    g.sample_design();
    show_panel(&mut g, "art_library");
    g.exec("art.new", json!({}));
    g.exec("art.new", json!({}));
    g.app().extras.art.pending = Some((1, 0, Item { name: "Box".into(), payload: String::new() }));
    g.frames(2);
    assert!(g.has_text("Replace it"));
    // The first library closes: the question follows its own library.
    g.click_label_nth("Close", 0);
    assert_eq!(g.app().extras.art.pending.as_ref().map(|p| p.0), Some(0));
    g.click_label("Close");
    assert!(g.app().extras.art.libs.is_empty());
    assert!(!g.has_text("Replace it"), "the question outlived its library");
    assert!(g.app().extras.art.pending.is_none());
}

/// The Move panel works in one unit: with the inch setting on, a number typed into Move to is
/// in inches like the Distance next to it, the Position line names its unit, and a typed unit
/// or sum is worked out instead of being dropped.
#[test]
fn the_move_panel_positions_follow_the_inch_setting() {
    use crate::laser_panel::{position_text, typed_mm};
    let mut g = Gui::standard();
    show_panel(&mut g, "move");
    assert!(g.has_text("X ?   Y ?   Z ? mm   U ?"), "{:?}", g.visible_texts());
    // The panel is taller than its dock: bring the rest into view.
    let at = g.find_text("Get position").unwrap().center();
    let reveal = |g: &mut Gui| {
        g.scroll(at, egui::vec2(0.0, 4000.0));
        for _ in 0..60 {
            if g.find_text("Save position").is_some_and(|r| r.bottom() < g.screen().bottom() - 80.0) {
                return;
            }
            g.scroll(at, egui::vec2(0.0, -40.0));
        }
        panic!("Save position did not scroll into view");
    };
    reveal(&mut g);
    // Millimetres: a bare number, and a length with its unit.
    g.app().machine.jog.goto_x = "12.5".into();
    g.app().machine.jog.goto_y = "10in/4".into();
    g.click_label("Save position");
    let saved = g.app().machine.jog.saved.clone();
    assert_eq!((saved[0].x, saved[0].y, saved[0].use_z), (12.5, 63.5, false), "a typed sum with a unit was saved as 0");
    // The saved position's row fits the panel: the dock keeps its place and its width.
    g.frames(2);
    assert_eq!(g.layout_problems(), Vec::<String>::new(), "a saved position pushed the Move panel out of its dock");
    assert!(g.find_text("MOVE TO").is_some_and(|r| r.left() > 1221.0), "the Move panel slid under the canvas: {:?}", g.find_text("MOVE TO"));
    // Inches: the same fields now count in inches.
    g.app().prefs.numeric_inches = true;
    g.app().machine.jog.goto_x = "2".into();
    g.app().machine.jog.goto_y = "25.4 mm".into();
    g.app().machine.jog.goto_z = "0.5".into();
    g.frames(2);
    reveal(&mut g);
    g.click_label("Save position");
    let saved = g.app().machine.jog.saved.clone();
    assert!((saved[1].x - 50.8).abs() < 1e-9 && (saved[1].y - 25.4).abs() < 1e-9, "2 in was saved as {} mm", saved[1].x);
    assert!(saved[1].use_z && (saved[1].z - 12.7).abs() < 1e-9);

    g.scroll(at, egui::vec2(0.0, 2000.0));
    assert!(g.has_text("X ?   Y ?   Z ? in   U ?"), "{:?}", g.visible_texts());
    assert_eq!(g.layout_problems(), Vec::<String>::new());

    assert_eq!(typed_mm("", true), None);
    assert_eq!(typed_mm("abc", false), None);
    assert_eq!(typed_mm("-3,5", false), Some(-3.5));
    assert_eq!(position_text(Some([25.4, 50.8, 2.54, 7.0]), true), "X 1.000   Y 2.000   Z 0.100 in   U 7.000");
    assert_eq!(position_text(Some([25.4, 50.8, 2.54, 7.0]), false), "X 25.400   Y 50.800   Z 2.540 mm   U 7.000");
}

/// A saved position's row (name, Z, Go, up, down, Delete) fits the Move panel: it used to be one
/// line wider than the dock, which slid the whole dock's contents under the canvas.
#[test]
fn a_saved_position_does_not_push_the_move_panel_out_of_its_dock() {
    let mut g = Gui::standard();
    show_panel(&mut g, "move");
    let dock = g.rect_of("panel:area:RightBottom").unwrap();
    let left = |g: &Gui, label: &str| g.find_text(label).unwrap_or_else(|| panic!("{label} is on screen")).left();
    let before = left(&g, "POSITION");
    assert!(before > dock.left());
    g.app().machine.jog.saved.push(crate::machine_ui::SavedPos { name: "Corner jig".into(), x: 1.0, y: 2.0, z: 0.0, use_z: false });
    let at = g.find_text("Get position").unwrap().center();
    for _ in 0..60 {
        if g.find_text("Delete").is_some_and(|r| r.top() > dock.top() + 40.0 && r.bottom() < dock.bottom() - 20.0) && g.has_text("SAVED POSITIONS") {
            break;
        }
        g.scroll(at, egui::vec2(0.0, -30.0));
    }
    assert_eq!(g.rect_of("panel:area:RightBottom"), Some(dock));
    assert_eq!(left(&g, "SAVED POSITIONS"), before, "the panel's contents slid sideways");
    // Every control of the row is inside the dock and can be used.
    for label in ["Go", "Delete", "Save position"] {
        let r = g.find_text(label).unwrap_or_else(|| panic!("{label} is on screen"));
        assert!(r.left() >= dock.left() && r.right() <= dock.right(), "{label} is outside the dock: {r:?}");
    }
    g.click_label("Delete");
    assert!(g.app().machine.jog.saved.is_empty(), "Delete in the row could not be clicked");
}
