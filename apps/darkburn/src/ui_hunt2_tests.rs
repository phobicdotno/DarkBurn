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
