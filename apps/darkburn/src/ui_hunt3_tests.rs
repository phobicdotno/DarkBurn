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

/// Create QR Code with Barcode picked: OK waits for data the barcode can hold. It used to close
/// the window and leave an empty code behind.
#[test]
fn the_barcode_dialog_keeps_ok_for_data_the_code_can_hold() {
    let mut g = Gui::standard();
    g.run_command("tools.qr");
    g.settle();
    g.click_label("Barcode");
    g.frames(2);
    g.click_id("dialog:ok");
    g.frames(3);
    assert!(g.app().text.qr.open, "OK closed the window with nothing to encode");
    assert_eq!(g.app().editor.doc.len(), 0, "a barcode with no data was added");
    let at = field_in_window(&g, "TextInput", "Data");
    g.click_pos(at);
    g.type_text("AB-123");
    g.frames(2);
    g.click_id("dialog:ok");
    g.frames(3);
    assert!(!g.app().text.qr.open, "OK works once the data fits: {:?}", g.visible_texts());
    assert_eq!(g.app().editor.doc.len(), 1);
}

/// Focus Test and Interval Test: the numbers in the window are the window's own until Create.
/// Close keeps nothing; Create stores them with the project. A whole-number field takes a sum.
#[test]
fn the_test_windows_keep_their_numbers_to_themselves_until_create() {
    let mut g = Gui::standard();
    g.frames(2);
    g.app().editor.mark_saved();
    let state = |g: &Gui| {
        let app = g.app();
        (app.vision.state.focus, app.vision.state.interval, app.editor.is_modified())
    };
    let before = state(&g);

    g.run_command("laser.focus_test");
    g.settle();
    type_into(&mut g, "SpinButton", "Z start", "-7");
    type_into(&mut g, "SpinButton", "Lines", "10 + 5");
    g.frames(2);
    assert!(g.app().vision.state.focus == before.0, "typing in the window changed the project's setup");
    g.click_label("Close");
    g.frames(3);
    assert!(!g.dialog_open());
    assert!(state(&g) == before, "Close kept numbers that were only typed");
    // Opened again it shows the stored numbers, not the ones typed last time.
    g.run_command("laser.focus_test");
    g.settle();
    type_into(&mut g, "SpinButton", "Lines", "10 + 5");
    g.click_label("Create");
    g.frames(3);
    assert_eq!(g.app().vision.state.focus.steps, 15, "the Lines field takes a sum and Create stores it");
    assert_eq!(g.app().vision.state.focus.z_start, before.0.z_start, "the Z start typed before Close came back");
    assert!(!g.app().editor.doc.is_empty(), "Create drew the test");
    g.click_label("Close");
    g.frames(3);

    g.run_command("laser.interval_test");
    g.settle();
    type_into(&mut g, "SpinButton", "Gap", "9");
    g.frames(2);
    assert!(g.app().vision.state.interval == before.1, "typing in the window changed the project's setup");
    g.click_label("Close");
    g.frames(3);
    assert!(g.app().vision.state.interval == before.1, "Close kept numbers that were only typed");
    g.run_command("laser.interval_test");
    g.settle();
    type_into(&mut g, "SpinButton", "Gap", "9");
    g.click_label("Create");
    g.frames(3);
    assert_eq!(g.app().vision.state.interval.gap, 9.0);
}

/// `shape.set_props` and `layer.delete` say `changed: false` and make no undo step when there
/// was nothing to change.
#[test]
fn commands_that_change_nothing_say_so() {
    let mut g = Gui::standard();
    g.sample_design();
    let steps = g.app().editor.history.len();
    let r = g.exec("shape.set_props", json!({}));
    assert_eq!(r["changed"], false, "{r}");
    let r = g.exec("shape.set_props", json!({ "locked": false }));
    assert_eq!(r["changed"], false, "what the shapes already had: {r}");
    let r = g.exec("layer.delete", json!({ "layer": 17 }));
    assert_eq!(r["changed"], false, "an unused layer: {r}");
    assert_eq!(g.app().editor.history.len(), steps, "undo steps for no change");
    let r = g.exec("shape.set_props", json!({ "name": "Lid" }));
    assert_eq!(r["changed"], true);
    let layer = g.app().editor.doc.iter().next().unwrap().layer;
    let r = g.exec("layer.delete", json!({ "layer": layer, "with_shapes": true }));
    assert_eq!(r["changed"], true);
    assert_eq!(g.app().editor.history.len(), steps + 2);
}

fn show_panel(g: &mut Gui, panel: &str) {
    if !g.app().prefs.panel(panel).visible {
        g.run_command(&format!("panel.{panel}"));
    }
    let area = g.app().prefs.panel(panel).area;
    g.app().prefs.active_tab.insert(area, panel.to_string());
    g.frames(3);
}

/// "Auto-advance after a job" does what it says: a job made from the project that finishes on
/// the machine moves the serial number on, and the canvas then shows the next one. The tick box
/// was stored and never looked at.
#[test]
fn a_finished_job_moves_variable_text_on_when_auto_advance_is_ticked() {
    let mut g = Gui::standard();
    connect_simulator(&mut g);
    g.exec("text.add", json!({ "text": "{serial}", "variable": "serial", "x": 50, "y": 50, "height": 10, "layer": 0 }));
    let run_job = |g: &mut Gui| {
        let finished = |g: &Gui| g.app().machine.job_log.iter().filter(|l| l.starts_with("Job finished")).count();
        let before = finished(g);
        g.run_command("job.start");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while finished(g) == before {
            assert!(std::time::Instant::now() < end, "the job finished; log {:?}, note {:?}", g.app().machine.job_log, g.app().note);
            std::thread::sleep(std::time::Duration::from_millis(10));
            g.frames(2);
        }
        g.frames(3);
    };
    let current = |g: &Gui| g.app().editor.doc.variables.serial.current;
    run_job(&mut g);
    assert_eq!(current(&g), 1, "auto-advance is off: the number stays");
    g.exec("variable.set", json!({ "auto_advance": true }));
    run_job(&mut g);
    assert_eq!(current(&g), 2, "the finished job did not move the serial number on");
    run_job(&mut g);
    assert_eq!(current(&g), 3);
    // A job that is stopped does not count.
    g.run_command("job.start");
    g.wait_for_job_build();
    g.run_command("job.stop");
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        g.frames(2);
    }
    assert!(current(&g) == 3 || g.app().machine.job_log.last().is_some_and(|l| l.starts_with("Job finished")), "a stopped job moved the number on");
}

/// The Variable Text panel: a value that is typed again as it was is no change (no undo step),
/// and the column-names box still works when the CSV file has gone: the rows are in the project.
#[test]
fn the_variable_text_panel_makes_no_undo_step_for_no_change_and_keeps_a_lost_csv_usable() {
    let mut g = Gui::standard();
    let steps = g.app().editor.history.len();
    let r = g.exec("variable.set", json!({ "start": 1, "padding": 0 }));
    assert_eq!(r["changed"], false, "{r}");
    let r = g.exec("variable.set", json!({}));
    assert_eq!(r["changed"], false, "{r}");
    assert_eq!(g.app().editor.history.len(), steps, "undo steps for no change");
    let r = g.exec("variable.set", json!({ "start": 5 }));
    assert_eq!(r["changed"], true);

    let file = g.temp("names.csv");
    std::fs::write(&file, "Name,Code\n\"Smith, John\",A1\nLee,\n").unwrap();
    g.exec("variable.csv_load", json!({ "path": file.display().to_string(), "header": false }));
    std::fs::remove_file(&file).unwrap();
    show_panel(&mut g, "variable_text");
    assert!(!g.app().editor.doc.variables.merge.header);
    g.click_label("First row holds column names");
    g.frames(2);
    let m = g.app().editor.doc.variables.merge.clone();
    assert!(m.header, "the box could not be ticked once the file was gone: {:?}", g.app().note);
    assert_eq!((m.row_count(), m.value(0, 0)), (2, Some("Smith, John")), "the rows kept in the project are still used");
}

/// Every window a command opens: what Tab reaches, as (role, label), and what went wrong.
fn tab_walk(g: &mut Gui, presses: usize) -> (Vec<String>, Vec<String>) {
    let mut reached = Vec::new();
    let mut problems = Vec::new();
    for n in 0..presses {
        g.key(Key::Tab, Modifiers::NONE);
        g.frames(2);
        let Some(f) = g.ctx().memory(|m| m.focused()).map(|i| i.value()) else {
            reached.push("nothing".to_string());
            continue;
        };
        let windows: Vec<egui::Rect> = g.dialog_windows();
        match g.a11y().into_iter().find(|n| n.id == f) {
            Some(node) => {
                if !windows.iter().any(|w| w.expand(4.0).contains(node.rect.center())) {
                    problems.push(format!("press {n}: the keyboard is on {} {:?} at {:?}, outside the window", node.role, node.label, node.rect));
                }
                reached.push(format!("{} {:?}", node.role, node.label.unwrap_or_default()));
            }
            None => reached.push("unnamed".to_string()),
        }
    }
    (reached, problems)
}

/// With a dialog open, Tab walks the dialog's own controls: the first press lands in it, every
/// press stays in it, and nothing behind it is touched. The walk used to start on the
/// application bar behind the dialog.
#[test]
fn tab_stays_in_the_open_dialog() {
    let mut g = Gui::standard();
    let ids: Vec<&str> = crate::commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    let mut seen = 0;
    let mut all = Vec::new();
    for id in ids {
        g.close_dialogs();
        crate::files::new_project(&mut g.app());
        g.sample_design();
        g.run_command(id);
        if g.dialog_windows().is_empty() {
            continue;
        }
        seen += 1;
        let doc = g.app().editor.doc.clone();
        let (reached, problems) = tab_walk(&mut g, 14);
        all.extend(problems.into_iter().map(|p| format!("{id}: {p}")));
        if reached.iter().all(|r| r == "nothing") {
            all.push(format!("{id}: Tab reaches nothing in the window"));
        }
        if g.app().editor.doc != doc {
            all.push(format!("{id}: Tab changed the drawing behind the window"));
        }
        eprintln!("{id}: {reached:?}");
    }
    assert!(seen >= 15, "only {seen} dialogs were opened");
    assert!(all.is_empty(), "{} problems:\n{}", all.len(), all.join("\n"));
}

/// Tab until the keyboard is on the button `label`.
fn tab_to_button(g: &mut Gui, label: &str) {
    for _ in 0..60 {
        g.key(Key::Tab, Modifiers::NONE);
        g.frames(2);
        let focused = g.ctx().memory(|m| m.focused()).map(|i| i.value());
        if g.a11y().into_iter().any(|n| Some(n.id) == focused && n.role == "Button" && n.label.as_deref() == Some(label)) {
            return;
        }
    }
    panic!("Tab never reached the button {label}");
}

/// Enter presses the button that holds the keyboard, also in a window with fields: there Enter
/// on Cancel or on the OK button did nothing at all (Space worked). Enter that ends typing in a
/// field still does not press OK.
#[test]
fn enter_presses_the_focused_button_of_a_dialog_with_fields() {
    let open = |g: &mut Gui| {
        g.close_dialogs();
        crate::files::new_project(&mut g.app());
        g.sample_design();
        g.run_command("edit.convert_bitmap");
        assert!(g.app().images.convert.is_some());
    };
    let images = |g: &Gui| g.app().editor.doc.iter().filter(|s| s.is_image()).count();
    let mut g = Gui::standard();
    open(&mut g);
    tab_to_button(&mut g, "Cancel");
    g.enter();
    g.frames(3);
    assert!(g.app().images.convert.is_none(), "Enter on Cancel left the window open");
    assert_eq!(images(&g), 0, "Enter on Cancel converted the shapes");

    open(&mut g);
    tab_to_button(&mut g, "Convert");
    g.enter();
    g.frames(3);
    assert!(g.app().images.convert.is_none(), "Enter on Convert left the window open");
    assert_eq!(images(&g), 1, "Enter on Convert did not convert");

    // Enter that ends typing in a field is not OK.
    open(&mut g);
    type_into(&mut g, "SpinButton", "DPI", "100");
    g.frames(2);
    assert!(g.app().images.convert.as_ref().is_some_and(|d| d.args.dpi == 100.0), "the typed DPI is in the window, and it is still open");
    assert_eq!(images(&g), 0);
    g.close_dialogs();
}

/// A field in a panel that was clicked into keeps the keyboard and its Tab order while a window
/// is open beside it: Tab moves on to the next panel field and does not jump into the window.
#[test]
fn tab_in_a_panel_field_moves_on_while_a_window_is_open() {
    let mut g = Gui::standard();
    g.sample_design();
    g.run_command("laser.focus_test");
    g.settle();
    let window = g.find_a11y(Some("Window"), "Focus Test").into_iter().next().expect("the window is open").rect;
    g.click_id("dialog:control:Speed");
    g.frames(2);
    let first = g.ctx().memory(|m| m.focused());
    assert!(first.is_some(), "the click went into the Speed field of the panel");
    g.key(Key::Tab, Modifiers::NONE);
    g.frames(2);
    let second = g.ctx().memory(|m| m.focused());
    assert!(second.is_some() && second != first, "Tab did not move on from the panel field: {first:?} then {second:?}");
    let node = g.a11y().into_iter().find(|n| Some(n.id) == second.map(|i| i.value()));
    assert!(node.as_ref().is_none_or(|n| !window.contains(n.rect.center())), "Tab jumped from the panel into the window: {node:?}");
    // With the keyboard nowhere, Tab goes into the window.
    g.escape();
    g.frames(2);
    if g.find_a11y(Some("Window"), "Focus Test").is_empty() {
        g.run_command("laser.focus_test");
        g.settle();
    }
    g.ctx().memory_mut(|m| m.stop_text_input());
    g.frames(1);
    g.key(Key::Tab, Modifiers::NONE);
    g.frames(2);
    let focused = g.ctx().memory(|m| m.focused()).map(|i| i.value());
    let node = g.a11y().into_iter().find(|n| Some(n.id) == focused);
    let window = g.find_a11y(Some("Window"), "Focus Test").into_iter().next().expect("the window is open").rect;
    assert!(node.as_ref().is_some_and(|n| window.contains(n.rect.center())), "Tab did not go into the window: {node:?}");
    g.close_dialogs();
}
