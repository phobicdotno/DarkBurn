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

/// The position and size fields of the ribbon are named after their labels (X, Y, W, H ...), not
/// after their units, and take sums and units.
#[test]
fn the_ribbon_number_fields_are_named_and_take_sums_and_units() {
    let mut g = Gui::standard();
    g.sample_design();
    let first = g.app().editor.doc.iter().next().unwrap().id;
    g.app().editor.selection.set([first]);
    g.frames(2);
    let ids = g.app().editor.selected();
    for label in ["X", "Y", "W", "H", "W %", "H %", "Rotate"] {
        assert_eq!(g.find_a11y(Some("SpinButton"), label).len(), 1, "a number field named {label}");
    }
    for unit in ["mm", "in", "%", "deg"] {
        assert!(g.find_a11y(Some("SpinButton"), unit).is_empty(), "a number field is named after its unit {unit}");
    }
    g.app().prefs.lock_aspect = false;
    g.click_a11y(Some("SpinButton"), "W");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("10in/4");
    g.enter();
    let bb = g.app().editor.doc.bbox_of(&ids).unwrap();
    assert!((bb.width() - 63.5).abs() < 1e-6 && (bb.height() - 50.0).abs() < 1e-6, "{bb:?}");
    g.click_a11y(Some("SpinButton"), "H");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("2cm + 5");
    g.enter();
    let bb = g.app().editor.doc.bbox_of(&ids).unwrap();
    assert!((bb.height() - 25.0).abs() < 1e-6, "{bb:?}");
}

/// A plain key pressed while a dialog is open belongs to the dialog: the arrows, Delete,
/// Backspace, Tab and the letter keys leave the drawing behind it alone.
#[test]
fn plain_keys_over_an_open_dialog_do_not_edit_the_drawing() {
    for window in ["help.about", "edit.settings", "tools.offset"] {
        let mut g = Gui::standard();
        g.sample_design();
        let state = |g: &Gui| {
            let app = g.app();
            (app.editor.doc.clone(), app.editor.selected())
        };
        let before = state(&g);
        g.run_command(window);
        assert!(g.dialog_open(), "{window} opened a window");
        for key in [Key::ArrowRight, Key::ArrowUp, Key::H, Key::V, Key::L, Key::Comma, Key::Tab, Key::PageUp, Key::Backspace, Key::Delete] {
            g.key(key, Modifiers::NONE);
            assert!(state(&g) == before, "{window}: {key:?} changed the drawing behind the window");
        }
        // With the window closed the keys are the canvas's again.
        g.close_dialogs();
        g.key(Key::ArrowRight, Modifiers::NONE);
        assert!(g.app().editor.doc != before.0, "{window}: the arrow nudges again once the window is closed");
        g.key(Key::Delete, Modifiers::NONE);
        assert_eq!(g.app().editor.doc.len(), 0);
    }
    // Keys with Ctrl and the function keys still work over a window: undo, and help for it.
    let mut g = Gui::standard();
    g.sample_design();
    let shapes = g.app().editor.doc.len();
    g.run_command("edit.settings");
    g.key(Key::Z, Modifiers::COMMAND);
    assert!(g.app().editor.doc.len() < shapes, "Ctrl+Z still undoes");
    g.key(Key::F1, Modifiers::NONE);
    assert!(g.app().extras.help_open, "F1 still opens the help");
}

/// The canvas's right-click menu: Preview opens the preview, and each item shows the key it
/// has now, not the default it was given.
#[test]
fn the_canvas_menu_opens_the_preview_and_shows_the_keys_in_use() {
    let mut g = Gui::standard();
    g.sample_design();
    g.exec("hotkeys.set", serde_json::json!({ "command": "edit.copy", "key": "Ctrl+Alt+C" }));
    let empty = g.mm_to_screen(300.0, 300.0);
    g.right_click_pos(empty);
    assert!(g.has_text("Ctrl+Alt+C"), "the menu shows the key Copy has now: {:?}", g.visible_texts());
    assert!(!g.has_text("Ctrl+C"));
    g.click_label("Preview");
    g.frames(2);
    assert!(g.app().job.preview.open, "Preview in the canvas menu opens the preview");
}

/// The status bar gives the selection's corners in machine coordinates, like the pointer
/// position next to it and the X and Y fields, wherever the origin is.
#[test]
fn the_status_bar_gives_the_selection_in_machine_coordinates() {
    let status = |g: &Gui| g.visible_texts().into_iter().find(|t| t.starts_with("Min ")).expect("the selection is in the status bar");
    let mut g = Gui::standard();
    g.sample_design();
    let bb = g.app().editor.doc.bbox().unwrap();
    assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (20.0, 20.0, 200.0, 70.0));
    assert!(status(&g).starts_with("Min 20.00, 20.00  Max 200.00, 70.00  (180.00 × 50.00 mm)"), "{}", status(&g));
    // Origin at the rear right of the 400 x 400 mm work area: X counts from the right, Y from the back.
    g.app().prefs.origin = crate::view::Origin::RearRight;
    g.frames(2);
    assert!(status(&g).starts_with("Min 200.00, 330.00  Max 380.00, 380.00  (180.00 × 50.00 mm)"), "{}", status(&g));
    // The pointer over the selection's front-left corner reads the same numbers.
    let corner = g.mm_to_screen(20.0, 400.0 - 20.0);
    g.hover_pos(corner);
    g.frames(2);
    let pointer = g.visible_texts().into_iter().find(|t| t.starts_with("x: ")).unwrap();
    let numbers: Vec<f64> = pointer.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    assert!((numbers[0] - 380.0).abs() < 1.0 && (numbers[1] - 380.0).abs() < 1.0, "{pointer}");
}

/// The mouse wheel over a window scrolls that window; it does not zoom the drawing behind it.
#[test]
fn the_wheel_over_a_window_does_not_zoom_the_canvas_behind_it() {
    let mut g = Gui::standard();
    g.sample_design();
    g.run_command("edit.settings");
    let window = g.dialog_windows().into_iter().next().expect("Settings is open");
    assert!(g.canvas_rect().contains(window.center()), "the window is over the canvas");
    let view = g.app().prefs.view;
    g.scroll(window.center(), egui::vec2(0.0, -120.0));
    g.scroll(window.center(), egui::vec2(0.0, 240.0));
    assert_eq!(g.app().prefs.view, view, "the wheel over Settings zoomed the canvas");
    // An open menu is in front of the canvas too.
    g.close_dialogs();
    let empty = g.mm_to_screen(300.0, 300.0);
    g.right_click_pos(empty);
    let item = g.find_text("Select All").expect("the canvas menu is open").center();
    g.scroll(item, egui::vec2(0.0, 120.0));
    assert_eq!(g.app().prefs.view, view, "the wheel over a menu zoomed the canvas");
    g.escape();
    // Over the bare canvas the wheel zooms as before.
    g.scroll(empty, egui::vec2(0.0, 120.0));
    assert!(g.app().prefs.view.zoom > view.zoom);
}
