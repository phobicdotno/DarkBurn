//! Bug-hunt tests for the interface: fields, settings, keys and menus. See docs/UI-TESTING.md.
use egui::{Key, Modifiers};

use crate::ui_harness::Gui;

/// Clicking into a number field and leaving it again is not an edit: the value keeps the
/// decimals the field does not show, and there is nothing to undo.
#[test]
fn leaving_a_number_field_untouched_changes_nothing() {
    let mut g = Gui::standard();
    g.sample_design();
    let layer = 1;
    g.app().editor.current_layer = layer;
    g.app().editor.doc.layer_mut(layer).main_mut().speed = 16.666_666;
    g.frames(2);
    let steps = g.app().editor.history.len();
    for leave in [Key::Tab, Key::Enter] {
        g.click_id("dialog:control:Speed");
        g.key(leave, Modifiers::NONE);
        assert_eq!(g.app().editor.doc.layer(layer).main().speed, 16.666_666, "the speed was rounded to what the field shows");
        assert_eq!(g.app().editor.history.len(), steps, "an undo step was made for no change");
    }
    // Typing a value still sets it, also one that only differs past the shown decimals.
    g.click_id("dialog:control:Speed");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("16.66");
    g.enter();
    assert_eq!(g.app().editor.doc.layer(layer).main().speed, 16.66);
    assert_eq!(g.app().editor.history.len(), steps + 1);
}

/// A layer name is typed a key at a time in the Cut Settings Editor: a space in the middle stays,
/// and the field can be emptied to type another name.
#[test]
fn a_layer_name_with_a_space_can_be_typed_in_the_cut_settings_editor() {
    let mut g = Gui::standard();
    g.sample_design();
    g.app().cut_settings_open = Some(1);
    g.frames(3);
    g.click_a11y(Some("TextInput"), "Layer name");
    g.key(Key::A, Modifiers::COMMAND);
    for part in ["Top", " ", "cut"] {
        g.type_text(part);
    }
    assert_eq!(g.app().editor.doc.layer(1).name, "Top cut");
    g.key(Key::A, Modifiers::COMMAND);
    g.key(Key::Backspace, Modifiers::NONE);
    assert_eq!(g.app().editor.doc.layer(1).name, "Top cut", "an emptied field is not a name yet");
    g.type_text("Engrave");
    assert_eq!(g.app().editor.doc.layer(1).name, "Engrave");
    // Another layer picked in the list shows its own name, not what was typed for the first.
    g.click_label("02 C02");
    assert_eq!(g.app().editor.doc.layer(1).name, "Engrave");
    assert_eq!(g.app().editor.doc.layer(2).name, "C02");
    g.click_label("OK");
    assert_eq!(g.app().editor.doc.layer(1).name, "Engrave");
    g.key(Key::Z, Modifiers::COMMAND);
    assert_ne!(g.app().editor.doc.layer(1).name, "Engrave", "the editor's changes are one undo step");
}

/// Scroll the open window until the text `label` is well inside it.
fn scroll_into_view(g: &mut Gui, label: &str) {
    let window = g.dialog_windows().into_iter().next().expect("a window is open");
    for _ in 0..60 {
        if g.find_text(label).is_some_and(|r| r.top() > window.top() + 60.0 && r.bottom() < window.bottom() - 70.0) {
            return;
        }
        g.scroll(window.center(), egui::vec2(0.0, -60.0));
    }
    panic!("{label:?} did not scroll into view; texts: {:?}", g.visible_texts());
}

/// The Modes toolbar is shown or hidden from the Window menu, from its tick in Settings and
/// from "Show the rotary and cylinder toggles"; looking at the Display page changes nothing.
#[test]
fn the_display_settings_leave_the_modes_toolbar_as_it_was() {
    const TOGGLES: &str = "Show the rotary and cylinder toggles";
    let mut g = Gui::standard();
    g.run_command("toolbar.modes");
    assert!(!g.app().prefs.toolbar("modes"));
    g.run_command("edit.settings");
    g.click_label("Display");
    g.frames(3);
    assert!(!g.app().prefs.toolbar("modes"), "opening the Display settings showed the hidden toolbar again");
    scroll_into_view(&mut g, TOGGLES);
    g.click_label(TOGGLES);
    g.frames(3);
    assert!(g.app().prefs.toolbar("modes"), "the tick shows the toolbar");
    g.click_label(TOGGLES);
    g.frames(3);
    assert!(!g.app().prefs.toolbar("modes"), "and hides it");
    // The toolbar's own tick in the list below does the same, and stays as it was set.
    scroll_into_view(&mut g, "Modes");
    g.click_label("Modes");
    g.frames(3);
    assert!(g.app().prefs.toolbar("modes"));
    g.click_label("Modes");
    g.frames(3);
    assert!(!g.app().prefs.toolbar("modes"), "the Modes tick in the toolbar list came back on by itself");
    // The setting by name (scripts, agents) is the same switch.
    g.exec("setting.set", serde_json::json!({ "key": "show_rotary_toggles", "value": true }));
    assert!(g.app().prefs.toolbar("modes"));
}
