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

/// Looking at the Camera panel changes nothing: an overlay width that does not survive the trip
/// to percent and back to the last digit (0.9 mm of a 400 mm bed) used to be rewritten, and the
/// project then counted as changed.
#[test]
fn the_camera_panel_leaves_the_overlay_size_alone_until_it_is_edited() {
    let mut g = Gui::standard();
    {
        let mut app = g.app();
        app.vision.state.camera.overlay.width_mm = Some(0.9);
        app.vision.state.camera.overlay.height_mm = Some(0.9);
        crate::vision_cmds::commit(&mut app, "Camera settings", false);
        app.editor.mark_saved();
    }
    show_panel(&mut g, "camera");
    g.frames(3);
    assert!(g.has_text("Overlay") || g.has_text("OVERLAY"), "the Camera panel is shown");
    let o = g.app().vision.state.camera.overlay;
    assert_eq!((o.width_mm, o.height_mm), (Some(0.9), Some(0.9)), "the panel rewrote the overlay size");
    assert!(!g.app().editor.is_modified(), "showing the Camera panel changed the project");
    // A value typed into the field is still taken.
    g.click_a11y(Some("SpinButton"), "Width");
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("50");
    g.enter();
    assert_eq!(g.app().vision.state.camera.overlay.width_mm, Some(200.0));
    assert_eq!(g.app().vision.state.camera.overlay.height_mm, Some(0.9));
    assert!(g.app().editor.is_modified());
}

/// The shape dialogs with OK, Apply and Cancel: Cancel, Escape and the close button change
/// nothing and leave no preview on the canvas; OK is one undo step that undo takes back whole.
#[test]
fn shape_dialogs_cancel_cleanly_and_ok_is_one_undo_step() {
    for id in ["tools.offset", "arrange.grid_array", "arrange.circular_array", "arrange.copy_along_path", "arrange.nest"] {
        let mut g = Gui::standard();
        g.sample_design();
        let before = g.app().editor.doc.clone();
        let selected = g.app().editor.selected();
        let steps = g.app().editor.history.len();
        let untouched = |g: &Gui, how: &str| {
            let app = g.app();
            assert!(app.shape_ui.open.is_none(), "{id}: {how} left the dialog open");
            assert!(app.editor.doc == before, "{id}: {how} changed the drawing");
            assert_eq!(app.editor.history.len(), steps, "{id}: {how} made an undo step");
            assert_eq!(app.editor.selected(), selected, "{id}: {how} changed the selection");
            assert!(app.shape_ui.preview.is_empty() && app.shape_ui.highlights.is_empty(), "{id}: {how} left a preview on the canvas");
        };
        g.run_command(id);
        assert!(g.dialog_open(), "{id} opened a dialog");
        g.frames(2);
        if id != "arrange.nest" {
            assert!(!g.app().shape_ui.preview.is_empty(), "{id} shows a preview");
        }
        g.click_label("Cancel");
        untouched(&g, "Cancel");
        g.run_command(id);
        g.frames(2);
        g.escape();
        untouched(&g, "Escape");
        g.run_command(id);
        g.frames(2);
        g.click_label("OK");
        g.frames(2);
        {
            let app = g.app();
            assert!(app.shape_ui.open.is_none(), "{id}: OK left the dialog open: {:?}", app.shape_ui.error);
            assert!(app.editor.doc != before, "{id}: OK changed nothing");
            assert_eq!(app.editor.history.len(), steps + 1, "{id}: OK is one undo step");
            assert!(app.shape_ui.preview.is_empty(), "{id}: OK left a preview on the canvas");
        }
        g.key(Key::Z, Modifiers::COMMAND);
        assert!(g.app().editor.doc == before, "{id}: one undo did not take back the whole OK");
        g.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
        assert!(g.app().editor.doc != before, "{id}: redo brings it back");
    }
}

fn connect_simulator(g: &mut Gui) {
    g.run_command("machine.connect");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while g.app().machine.handle.snapshot().state != crate::machine_api::LaserState::Ready {
        assert!(std::time::Instant::now() < end, "the simulator connected");
        std::thread::sleep(std::time::Duration::from_millis(10));
        g.frames(2);
    }
}

/// The New Device wizard makes its device the one in use, and Remove on the device in use hands
/// over to another: a connection that was open belongs to the old device and is ended, so the
/// panels and the job builder never work for one machine while another is connected.
#[test]
fn a_new_or_removed_device_in_use_ends_the_open_connection() {
    use crate::devices::{DeviceType, Wizard, WizardStep};
    let mut g = Gui::standard();
    connect_simulator(&mut g);
    let first = g.app().machine.devices.active;
    let mut w = Wizard::new();
    w.set_type(DeviceType::SimulatorRuida);
    w.step = WizardStep::Summary;
    g.app().machine.dlg.wizard = Some(w);
    g.frames(2);
    g.settle();
    g.click_label("Add");
    g.frames(2);
    let added = g.app().machine.devices.active;
    assert_ne!(added, first, "the new device is the one in use");
    assert!(!g.app().machine.handle.is_connected(), "the first simulator is still connected though another device is in use");
    g.close_dialogs();

    // Remove the device in use while it is connected.
    connect_simulator(&mut g);
    g.run_command("laser.devices");
    g.frames(2);
    g.click_label("Remove");
    g.frames(2);
    assert_ne!(g.app().machine.devices.active, added, "the device in use was removed");
    assert!(!g.app().machine.handle.is_connected(), "the removed device is still connected");
    g.close_dialogs();
}

/// Scroll the open window until the text `label` is well inside it.
fn scroll_window_to(g: &mut Gui, label: &str) {
    let window = g.dialog_windows().into_iter().next().expect("a window is open");
    for _ in 0..80 {
        if g.find_text(label).is_some_and(|r| r.top() > window.top() + 60.0 && r.bottom() < window.bottom() - 70.0) {
            return;
        }
        g.scroll(window.center(), egui::vec2(0.0, -60.0));
    }
    // The last rows of a window stop just above its buttons: that is as far as it scrolls.
    assert!(g.find_text(label).is_some_and(|r| window.contains_rect(r)), "{label:?} did not scroll into view; texts: {:?}", g.visible_texts());
}

/// "Use current layers as defaults" in Device Settings belongs to the device being edited and
/// to its OK: it used to write to the device in use at once, whichever device the window was
/// for, and Cancel did not take it back.
#[test]
fn device_settings_default_layers_go_to_the_edited_device_on_ok_only() {
    const BUTTON: &str = "Use current layers as defaults";
    let mut g = Gui::standard();
    g.sample_design();
    g.app().editor.doc.layer_mut(1).main_mut().speed = 77.0;
    let in_use = g.app().machine.devices.active.unwrap();
    let other = g.app().machine.devices.devices.iter().find(|d| d.id != in_use).unwrap().clone();
    let defaults = |g: &Gui, id: u64| g.app().machine.devices.get(id).unwrap().state.default_layers.clone();
    let open = |g: &mut Gui| {
        g.app().machine.dlg.settings = Some((other.clone(), 0));
        g.settle();
        scroll_window_to(g, BUTTON);
        g.click_label(BUTTON);
    };
    open(&mut g);
    assert_eq!(defaults(&g, in_use), None, "the button wrote to the device in use, not the edited one");
    g.click_id("dialog:cancel");
    g.frames(3);
    assert!(g.app().machine.dlg.settings.is_none());
    assert_eq!((defaults(&g, in_use), defaults(&g, other.id)), (None, None), "Cancel kept the new default layers");
    open(&mut g);
    g.click_id("dialog:ok");
    g.frames(3);
    assert_eq!(defaults(&g, in_use), None);
    assert!(defaults(&g, other.id).is_some_and(|t| t.contains("77")), "OK stores the layers with the edited device");
    // Apply puts the edited device's defaults on the project, as one undo step.
    g.app().editor.doc.layer_mut(1).main_mut().speed = 5.0;
    let steps = g.app().editor.history.len();
    let other = g.app().machine.devices.get(other.id).unwrap().clone();
    g.app().machine.dlg.settings = Some((other, 0));
    g.settle();
    scroll_window_to(&mut g, "Apply default layers");
    g.click_label("Apply default layers");
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 77.0, "Apply used another device's defaults");
    assert_eq!(g.app().editor.history.len(), steps + 1);
}

/// "Home when a job starts" on the first page of Device Settings was kept with the device and
/// read by nothing: only the box of the same name on the G-code page did anything.
#[test]
fn device_settings_home_when_a_job_starts_on_the_first_page_reaches_the_job() {
    const BOX: &str = "Home when a job starts";
    let mut g = Gui::standard();
    let id = g.app().machine.devices.active.unwrap();
    let dev = g.app().machine.active().clone();
    assert!(!dev.settings.gcode.to_gcode().start_script.contains("$H"));
    g.app().machine.dlg.settings = Some((dev, 0));
    g.settle();
    scroll_window_to(&mut g, BOX);
    g.click_label(BOX);
    g.click_id("dialog:ok");
    g.frames(3);
    assert!(g.app().machine.dlg.settings.is_none());
    let dev = g.app().machine.devices.get(id).unwrap().clone();
    assert!(dev.settings.gcode.to_gcode().start_script.starts_with("$H"), "the job does not home: {:?}", dev.settings.gcode.to_gcode().start_script);
}

/// Starting to type a text and leaving again with nothing typed (or nothing changed) is not a
/// change to the project.
#[test]
fn a_text_that_was_not_typed_or_not_changed_leaves_a_saved_project_saved() {
    let mut g = Gui::standard();
    g.app().editor.mark_saved();
    g.app().set_tool(Tool::Text);
    g.frames(2);
    g.click_mm(100.0, 100.0);
    assert!(g.app().text.typing.is_some());
    g.escape();
    g.frames(2);
    assert_eq!(g.app().editor.doc.len(), 0);
    assert!(!g.app().editor.is_modified(), "a click with the text tool and Escape changed the project");
    // A text that exists: into it and out again without a change.
    g.click_mm(100.0, 100.0);
    g.type_text("ab");
    g.escape();
    g.frames(2);
    assert_eq!(g.app().editor.doc.len(), 1);
    g.app().editor.mark_saved();
    let steps = g.app().editor.history.len();
    g.app().set_tool(Tool::Text);
    g.frames(2);
    let at = g.app().editor.doc.bbox().unwrap().center();
    g.click_mm(at.x as f32, 400.0 - at.y as f32);
    assert!(g.app().text.typing.is_some(), "the click went into the text");
    g.escape();
    g.frames(2);
    assert_eq!(g.app().editor.history.len(), steps, "an undo step for no change");
    assert!(!g.app().editor.is_modified(), "going into a text and out again changed the project");
}

/// A value changed in the Cut Settings Editor and taken back with Cancel is not a change to the
/// project: a saved project stays saved. OK with a change makes it a changed project.
#[test]
fn cancel_in_the_cut_settings_editor_leaves_a_saved_project_saved() {
    let mut g = Gui::standard();
    g.sample_design();
    g.app().editor.mark_saved();
    let edit_speed = |g: &mut Gui, value: &str| {
        g.app().cut_settings_open = Some(1);
        g.settle();
        let window = g.dialog_windows().into_iter().next().expect("the editor is open");
        let field = g.find_a11y(Some("SpinButton"), "Speed").into_iter().find(|f| window.contains(f.rect.center())).expect("a Speed field");
        g.click_pos(field.rect.center());
        g.key(Key::A, Modifiers::COMMAND);
        g.type_text(value);
        g.enter();
    };
    edit_speed(&mut g, "55");
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 55.0, "the edit shows live");
    assert!(g.app().editor.is_modified());
    g.click_id("dialog:cancel");
    g.frames(3);
    assert!(g.app().cut_settings_open.is_none());
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 100.0);
    assert!(!g.app().editor.is_modified(), "Cancel put the value back but left the project changed");
    // Looked at and closed with OK: nothing changed.
    g.app().cut_settings_open = Some(1);
    g.settle();
    g.click_id("dialog:ok");
    g.frames(3);
    assert!(!g.app().editor.is_modified());
    // OK with a change is a change.
    edit_speed(&mut g, "66");
    g.click_id("dialog:ok");
    g.frames(3);
    assert!(g.app().editor.is_modified());
    g.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(g.app().editor.doc.layer(1).main().speed, 100.0);
}

fn field_in_window(g: &Gui, role: &str, label: &str) -> egui::Pos2 {
    let window = g.dialog_windows().into_iter().next().expect("a window is open");
    let found = g.find_a11y(Some(role), label).into_iter().find(|f| window.contains(f.rect.center()));
    found.unwrap_or_else(|| panic!("no {role} named {label} in the window: {:?}", g.a11y())).rect.center()
}

/// Create QR Code: Cancel and Escape add nothing; a size typed and confirmed with Enter is the
/// size of the code OK makes; OK is one undo step.
#[test]
fn the_qr_dialog_cancels_cleanly_and_ok_uses_what_was_typed() {
    let mut g = Gui::standard();
    let steps = g.app().editor.history.len();
    g.run_command("tools.qr");
    g.settle();
    assert!(g.dialog_open());
    g.click_id("dialog:cancel");
    g.frames(3);
    assert!(!g.app().text.qr.open && g.app().editor.doc.is_empty() && g.app().editor.history.len() == steps);
    g.run_command("tools.qr");
    g.settle();
    g.escape();
    g.frames(3);
    assert!(!g.app().text.qr.open && g.app().editor.doc.is_empty(), "Escape closes it and adds nothing");

    g.run_command("tools.qr");
    g.settle();
    let text = field_in_window(&g, "MultilineTextInput", "Text");
    g.click_pos(text);
    g.type_text("hello");
    let size = field_in_window(&g, "SpinButton", "Size");
    g.click_pos(size);
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("20 + 22");
    g.enter();
    if g.app().text.qr.open {
        g.click_id("dialog:ok");
    }
    g.frames(3);
    assert!(!g.app().text.qr.open, "OK closed the window");
    assert_eq!(g.app().editor.doc.len(), 1, "one code was made: {:?}", g.app().note);
    assert_eq!(g.app().editor.history.len(), steps + 1, "OK is one undo step");
    let bb = g.app().editor.doc.bbox().unwrap();
    assert!((bb.width() - 42.0).abs() < 0.5 && (bb.height() - 42.0).abs() < 0.5, "the code is {bb:?}, not the 42 mm that was typed");
    assert!((bb.center().x - 200.0).abs() < 0.5 && (bb.center().y - 200.0).abs() < 0.5, "{bb:?}");
    g.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(g.app().editor.doc.len(), 0);
}

/// The number fields of the panels and dialogs take sums and units like the ribbon's: a typed
/// sum, a length with its unit, and the field's own unit after the number. They used to keep
/// the old value without a word for anything but a plain number.
#[test]
fn panel_and_dialog_number_fields_take_sums_and_units() {
    let mut g = Gui::standard();
    show_panel(&mut g, "move");
    let set = |g: &mut Gui, label: &str, text: &str| {
        let dock = g.rect_of("panel:area:RightBottom").unwrap();
        let field = g.find_a11y(Some("SpinButton"), label).into_iter().find(|f| dock.contains(f.rect.center()));
        let at = field.unwrap_or_else(|| panic!("a field named {label} in the Move panel")).rect.center();
        g.click_pos(at);
        g.key(Key::A, Modifiers::COMMAND);
        g.type_text(text);
        g.enter();
    };
    let distance = |g: &Gui| g.app().machine.jog.distance;
    for (typed, mm) in [("20 + 22", 42.0), ("10in/4", 63.5), ("7 mm", 7.0), ("2,5", 2.5), ("3cm", 30.0)] {
        set(&mut g, "Distance", typed);
        assert!((distance(&g) - mm).abs() < 1e-9, "Distance {typed:?} gave {} mm, not {mm}", distance(&g));
    }
    // Not a number: the value stays.
    set(&mut g, "Distance", "abc");
    assert!((distance(&g) - 30.0).abs() < 1e-9);
    // A field that is not a length: a sum of plain numbers, and its unit typed after the number.
    set(&mut g, "Speed", "50 + 25");
    assert_eq!(g.app().machine.jog.speed, 75.0);
    set(&mut g, "Speed", "80 mm/s");
    assert_eq!(g.app().machine.jog.speed, 80.0);
    // A field shown in inches: a bare number is inches, a length is converted.
    g.app().prefs.numeric_inches = true;
    g.frames(2);
    set(&mut g, "Distance", "1/2");
    assert!((distance(&g) - 12.7).abs() < 1e-9, "{}", distance(&g));
    set(&mut g, "Distance", "50.8 mm");
    assert!((distance(&g) - 50.8).abs() < 1e-9, "{}", distance(&g));
    set(&mut g, "Distance", "2 in");
    assert!((distance(&g) - 50.8).abs() < 1e-9, "{}", distance(&g));
}

/// Rotary Setup: Plan test shows what the settings in the window would do and keeps nothing;
/// closing the window afterwards leaves the project's rotary setup (and Enable rotary) as it
/// was. Apply keeps them, as one undo step.
#[test]
fn planning_a_rotary_test_does_not_keep_the_settings() {
    let mut g = Gui::standard();
    g.app().editor.mark_saved();
    let steps = g.app().editor.history.len();
    let before = g.app().vision.state.rotary;
    assert!(!before.enabled);
    g.run_command("laser.rotary");
    g.settle();
    assert!(g.dialog_open());
    g.click_label("Enable rotary");
    let at = g.find_a11y(Some("SpinButton"), "Steps per rotation").into_iter().next().expect("the field is in the window").rect.center();
    g.click_pos(at);
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("12300 + 45");
    g.enter();
    assert!(g.has_text("12345"), "the field takes a sum: {:?}", g.visible_texts());
    g.click_label("Plan test");
    g.frames(2);
    assert!(g.visible_texts().iter().any(|t| t.starts_with("Turn axis")), "the plan is shown: {:?}", g.visible_texts());
    assert!(g.app().vision.state.rotary == before, "Plan test changed the project's rotary setup");
    assert_eq!(g.app().editor.history.len(), steps, "Plan test made an undo step");
    g.click_label("Close");
    g.frames(3);
    assert!(!g.dialog_open());
    assert!(g.app().vision.state.rotary == before && !g.app().editor.is_modified(), "closing the window kept settings that were only tried");
    // Apply keeps what the window shows.
    g.run_command("laser.rotary");
    g.settle();
    g.click_label("Enable rotary");
    g.click_label("Apply");
    g.frames(2);
    assert!(g.app().vision.state.rotary.enabled);
    assert_eq!(g.app().editor.history.len(), steps + 1);
    g.click_label("Close");
    g.frames(2);
    g.key(Key::Z, Modifiers::COMMAND);
    g.frames(2);
    assert!(!g.app().vision.state.rotary.enabled, "one undo takes Apply back");
}
