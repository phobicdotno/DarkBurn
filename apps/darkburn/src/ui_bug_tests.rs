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

// ----- accessible names -----

/// Every control in every panel, ribbon tab and window has an accessible name.
#[test]
fn every_control_in_every_window_has_an_accessible_name() {
    let mut g = Gui::standard();
    g.sample_design();
    let mut bad = std::collections::BTreeSet::new();
    let roles = ["SpinButton", "Button", "CheckBox", "TextInput", "MultilineTextInput", "ComboBox", "Slider", "Link", "RadioButton", "Switch"];
    let mut look = |g: &Gui, what: &str| {
        for n in g.a11y() {
            if roles.contains(&n.role.as_str()) && n.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                bad.insert(format!("{what}: {} value {:?} at {:?}", n.role, n.value, n.rect));
            }
        }
    };
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    for p in crate::panels::PANELS {
        let area = g.app().prefs.panel(p.id).area;
        g.app().prefs.active_tab.insert(area, p.id.to_string());
        g.frames(3);
        look(&g, &format!("panel {}", p.id));
    }
    g.run_command("window.reset_layout");
    for tab in ["home", "arrange", "modify", "text", "modes"] {
        g.click_id(&format!("ribbon:tab:{tab}"));
        look(&g, &format!("ribbon {tab}"));
    }
    let ids: Vec<&str> = commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    for id in ids {
        g.close_dialogs();
        crate::files::new_project(&mut g.app());
        g.sample_design();
        g.run_command(id);
        if g.dialog_open() {
            look(&g, &format!("cmd {id} {:?}", g.window_titles()));
        }
    }
    for (what, open) in crate::ui_smoke_tests::state_dialog_cases() {
        g.close_dialogs();
        open(&mut g.app());
        g.frames(3);
        look(&g, &format!("state {what}"));
    }
    assert!(bad.is_empty(), "{} controls have no accessible name:\n{}", bad.len(), bad.into_iter().collect::<Vec<_>>().join("\n"));
}

/// A number field is named after its label, not after its unit, and a search box after its hint.
#[test]
fn fields_are_named_after_their_visible_labels() {
    let mut g = Gui::standard();
    g.sample_design();
    for p in ["art_library", "camera", "variable_text"] {
        if !g.app().prefs.panel(p).visible {
            g.run_command(&format!("panel.{p}"));
        }
    }
    let named = |g: &Gui, role: &str, label: &str| !g.find_a11y(Some(role), label).is_empty();
    for label in ["Speed", "Max power", "Min power", "Interval", "Passes"] {
        assert!(named(&g, "SpinButton", label), "Cuts / Layers has a number field named {label}");
    }
    assert!(g.find_a11y(Some("SpinButton"), "mm/s").is_empty(), "no field is named after its unit");
    assert!(named(&g, "TextInput", "Search art"));
    for (panel, role, label) in [
        ("console", "TextInput", "Console command"),
        ("camera", "Slider", "Fade"),
        ("camera", "SpinButton", "Exposure"),
        ("variable_text", "SpinButton", "Start"),
        ("shape_properties", "TextInput", "Name"),
        ("library", "TextInput", "Search the library"),
        ("library", "TextInput", "Library name"),
    ] {
        let area = g.app().prefs.panel(panel).area;
        g.app().prefs.active_tab.insert(area, panel.to_string());
        g.frames(3);
        let have: Vec<_> = g.a11y().into_iter().filter(|n| n.role == role).map(|n| n.label.unwrap_or_default()).collect();
        assert!(named(&g, role, label), "{panel} has a {role} named {label}; it has {have:?}");
    }
    g.run_command("window.preview");
    assert!(named(&g, "Slider", "Time in the job"));
    g.close_dialogs();
    g.run_command("laser.print_and_cut");
    assert!(named(&g, "SpinButton", "First target laser X") && named(&g, "SpinButton", "Second target design Y"));
}

// ----- widget IDs -----

/// Everything a layout shows that could reuse a widget id: the frame itself, each dock's tabs
/// with their right-click menus and overflow lists, and every ribbon tab.
fn clashes_in_layout(g: &mut Gui, what: &str, out: &mut Vec<String>) {
    let mut note = |g: &Gui, step: &str| {
        for c in g.id_clashes_now() {
            out.push(format!("{what}, {step}: {c}"));
        }
    };
    g.frames(3);
    note(g, "as shown");
    let tabs: Vec<(String, egui::Rect)> = g.rects("panel:tab:");
    for (name, r) in tabs {
        g.click_pos(r.center());
        note(g, &format!("{name} in front"));
        g.right_click_pos(r.center());
        note(g, &format!("menu of {name}"));
        g.escape();
    }
    for (name, r) in g.rects("panel:overflow:") {
        g.click_pos(r.center());
        note(g, &format!("list of {name}"));
        g.escape();
    }
    for (name, r) in g.rects("ribbon:tab:") {
        g.click_pos(r.center());
        note(g, &name);
    }
    for (name, r) in g.rects("") {
        g.hover_pos(r.center());
        note(g, &format!("pointer on {name}"));
    }
}

/// egui paints a warning when two widgets in one frame share an id (a click or a drag can then go
/// to the wrong one). None may show in the default layout, with every panel shown, with the
/// panels moved to each dock, or at the smallest window.
#[test]
fn no_widget_id_is_used_twice_in_any_panel_arrangement() {
    use crate::panels::{Area, PANELS};
    let mut found = Vec::new();
    for (w, h) in [(1600.0, 1000.0), (960.0, 600.0)] {
        let mut g = Gui::new(w, h, 1.0);
        g.allow_id_clashes();
        g.sample_design();
        clashes_in_layout(&mut g, &format!("default layout {w}x{h}"), &mut found);
        for p in PANELS {
            if !g.app().prefs.panel(p.id).visible {
                g.run_command(&format!("panel.{}", p.id));
            }
        }
        clashes_in_layout(&mut g, &format!("every panel shown {w}x{h}"), &mut found);
        // Every panel in one dock, for each dock: the most tabs a dock can hold.
        for area in Area::ALL {
            for p in PANELS {
                g.app().prefs.panels.insert(p.id.to_string(), crate::prefs::PanelState { area, visible: true });
            }
            clashes_in_layout(&mut g, &format!("all panels in {area:?} {w}x{h}"), &mut found);
        }
        // The panels dealt round the docks, shifted one dock each time.
        for shift in 0..Area::ALL.len() {
            for (i, p) in PANELS.iter().enumerate() {
                let area = Area::ALL[(i + shift) % Area::ALL.len()];
                g.app().prefs.panels.insert(p.id.to_string(), crate::prefs::PanelState { area, visible: true });
            }
            clashes_in_layout(&mut g, &format!("panels dealt round the docks ({shift}) {w}x{h}"), &mut found);
        }
        g.run_command("window.reset_layout");
        clashes_in_layout(&mut g, &format!("after a layout reset {w}x{h}"), &mut found);
    }
    found.sort();
    found.dedup();
    assert!(found.is_empty(), "{} widget id clashes:\n{}", found.len(), found.join("\n"));
}
