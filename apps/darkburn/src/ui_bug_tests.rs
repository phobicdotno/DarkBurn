//! UI bug-hunt tests (Escape, widget IDs, window sizes). See docs/UI-TESTING.md.
use crate::commands;
use crate::ui_harness::Gui;

// ----- Escape closes every window -----

/// Every window a command opens closes with one press of Escape, whatever tool is active.
#[test]
fn escape_closes_every_window_a_command_opens() {
    let mut g = Gui::standard();
    let ids: Vec<&str> = commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    let mut stuck = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        g.close_dialogs();
        crate::files::new_project(&mut g.app());
        g.sample_design();
        g.run_command(id);
        if !g.dialog_open() {
            continue;
        }
        let titles = g.window_titles();
        seen.extend(titles.iter().cloned());
        g.escape();
        if g.dialog_open() {
            stuck.push(format!("{id}: {titles:?} is still open: {:?}", g.window_titles()));
        }
    }
    assert!(stuck.is_empty(), "Escape did not close {} windows:\n{}", stuck.len(), stuck.join("\n"));
    for want in ["Preview", "Project notes", "Create QR Code", "Move Selection", "Print and Cut", "Rotary Setup", "Offset Shapes", "Settings"] {
        assert!(seen.contains(want), "{want} was not opened: {seen:?}");
    }
}

/// A canvas tool that uses Escape itself (nodes, measure, the pick tools) leaves the key to an
/// open window: the first press closes the window and the tool stays.
#[test]
fn escape_goes_to_the_window_before_the_canvas_tool() {
    for tool in ["tools.nodes", "tools.measure", "tools.tabs", "tools.line", "tools.text"] {
        for window in ["window.preview", "tools.qr", "arrange.move_selection", "laser.rotary", "help.about"] {
            let mut g = Gui::standard();
            g.sample_design();
            g.run_command(tool);
            let picked = g.app().tool;
            g.run_command(window);
            assert!(g.dialog_open(), "{window} opened a window");
            g.escape();
            assert!(!g.dialog_open(), "{tool} + {window}: one Escape closes {:?}", g.window_titles());
            assert_eq!(g.app().tool, picked, "{tool} + {window}: the tool stays");
        }
    }
}

#[test]
fn escape_closes_the_preview_and_stops_its_playback() {
    let mut g = Gui::standard();
    g.sample_design();
    g.click_id("ribbon:window.preview");
    assert!(g.app().job.preview.open);
    assert_eq!(g.window_titles(), ["Preview"]);
    g.app().job.preview.playing = true;
    g.escape();
    assert!(!g.app().job.preview.open && !g.app().job.preview.playing);
    assert!(!g.dialog_open());
    // With nothing open, the next Escape is the canvas's again: it clears the selection.
    assert!(!g.app().editor.selection.is_empty());
    g.escape();
    assert!(g.app().editor.selection.is_empty());
}

/// A dialog over the Preview takes Escape first; the Preview closes on the next press.
#[test]
fn escape_closes_one_window_at_a_time() {
    let mut g = Gui::standard();
    g.sample_design();
    g.run_command("window.preview");
    g.run_command("help.about");
    let mut titles = g.window_titles();
    titles.sort();
    assert_eq!(titles, ["About DarkBurn", "Preview"]);
    g.escape();
    assert_eq!(g.window_titles(), ["Preview"]);
    g.escape();
    assert!(!g.dialog_open());
}

/// While Print and Cut waits for a click on the canvas, Escape stops the pick and the window stays.
#[test]
fn escape_stops_a_position_pick_before_it_closes_the_window() {
    const PICKING: &str = "Click on the canvas to take the position";
    let mut g = Gui::standard();
    g.run_command("laser.print_and_cut");
    assert_eq!(g.window_titles(), ["Print and Cut"]);
    g.click_label("Pick");
    assert!(g.has_text(PICKING), "{:?}", g.visible_texts());
    g.escape();
    assert!(!g.has_text(PICKING), "the pick stopped");
    assert_eq!(g.window_titles(), ["Print and Cut"], "the first Escape only stops the pick");
    g.escape();
    assert!(!g.dialog_open());
}

#[test]
fn escape_closes_menus_and_drop_down_lists_before_anything_else() {
    let mut g = Gui::standard();
    g.sample_design();
    // An application-bar menu.
    g.open_menu_only("Tools");
    assert!(g.has_text("Offset Shapes"));
    g.escape();
    assert!(!g.has_text("Offset Shapes"), "the menu closed");
    assert!(!g.app().editor.selection.is_empty(), "the Escape that closed the menu did not reach the canvas");
    // The tab overflow list of a dock.
    g.click_id("panel:overflow:RightTop");
    assert!(g.has_text("Shape Properties"));
    g.escape();
    assert!(!g.has_text("Shape Properties"));
    // A drop-down list inside a window: the list closes, the window stays.
    g.run_command("window.preview");
    g.click_label("10x");
    assert!(g.has_text("500x"), "{:?}", g.visible_texts());
    g.escape();
    assert!(!g.has_text("500x"));
    assert_eq!(g.window_titles(), ["Preview"]);
    g.escape();
    assert!(!g.dialog_open());
}

/// The windows that open from state alone (messages, wizards, settings) close with Escape too.
#[test]
fn escape_closes_every_state_driven_window() {
    let mut g = Gui::standard();
    let mut stuck = Vec::new();
    for (what, open) in crate::ui_smoke_tests::state_dialog_cases() {
        g.close_dialogs();
        open(&mut g.app());
        g.frames(3);
        assert!(g.dialog_open(), "{what}: no window opened");
        g.escape();
        if g.dialog_open() {
            stuck.push(format!("{what}: {:?}", g.window_titles()));
        }
    }
    assert!(stuck.is_empty(), "Escape did not close: {stuck:?}");
}
