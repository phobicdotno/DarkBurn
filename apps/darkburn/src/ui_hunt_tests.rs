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
