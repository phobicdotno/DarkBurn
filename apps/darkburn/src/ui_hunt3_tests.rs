//! Third bug-hunt pass over the interface: saved state, device windows, test windows, fields.
//! See docs/UI-TESTING.md.
use egui::{Key, Modifiers};
use serde_json::json;

use crate::ui_harness::Gui;

fn connect_simulator(g: &mut Gui) {
    g.run_command("machine.connect");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while g.app().machine.handle.snapshot().state != crate::machine_api::LaserState::Ready {
        assert!(std::time::Instant::now() < end, "the simulator connected");
        std::thread::sleep(std::time::Duration::from_millis(10));
        g.frames(2);
    }
}

/// A named field of the window on top.
fn field_in_window(g: &Gui, role: &str, label: &str) -> egui::Pos2 {
    // The camera and test windows are plain windows: they have no dialog frame to look inside.
    let window = g.dialog_windows().into_iter().next();
    let found = g.find_a11y(Some(role), label).into_iter().find(|f| window.is_none_or(|w| w.contains(f.rect.center())));
    found.unwrap_or_else(|| panic!("no {role} named {label} in the window: {:?}", g.a11y())).rect.center()
}

fn type_into(g: &mut Gui, role: &str, label: &str, text: &str) {
    let at = field_in_window(g, role, label);
    g.click_pos(at);
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text(text);
    g.enter();
}

/// Undo back to what was saved is the saved project again: no star in the title, no question on
/// closing. Redo away from it is a change again.
#[test]
fn undo_back_to_the_saved_state_is_a_saved_project() {
    let mut g = Gui::standard();
    g.sample_design();
    g.app().editor.mark_saved();
    g.exec("shape.add_rect", json!({ "x": 10, "y": 10, "width": 40, "height": 20 }));
    g.frames(2);
    assert!(g.app().editor.is_modified());
    g.key(Key::Z, Modifiers::COMMAND);
    assert!(!g.app().editor.is_modified(), "undo back to the saved state still counts as a changed project");
    g.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(g.app().editor.is_modified(), "redo is a change again");
    g.key(Key::Z, Modifiers::COMMAND);
    assert!(!g.app().editor.is_modified());
}

/// Device Settings, Additional page: "Read from the controller" belongs to the connected
/// machine. A window that edits another device does not offer the connected machine's numbers.
#[test]
fn read_from_the_controller_is_only_offered_for_the_connected_device() {
    const BUTTON: &str = "Read from the controller";
    let mut g = Gui::standard();
    connect_simulator(&mut g);
    let in_use = g.app().machine.devices.active.unwrap();
    let mine = g.app().machine.devices.get(in_use).unwrap().clone();
    let other = g.app().machine.devices.devices.iter().find(|d| d.id != in_use).unwrap().clone();
    g.app().machine.dlg.settings = Some((mine, 3));
    g.settle();
    assert!(g.has_text(BUTTON), "the connected device can read its numbers: {:?}", g.visible_texts());
    g.click_id("dialog:cancel");
    g.frames(3);
    g.app().machine.dlg.settings = Some((other.clone(), 3));
    g.settle();
    assert!(g.has_text("Cut acceleration"), "the Additional page is shown");
    assert!(!g.has_text(BUTTON), "{} is offered the numbers of the connected machine", other.name);
    g.click_id("dialog:cancel");
    g.frames(3);
}
