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
