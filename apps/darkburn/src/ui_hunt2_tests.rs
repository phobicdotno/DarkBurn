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
