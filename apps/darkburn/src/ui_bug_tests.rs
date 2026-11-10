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

// ----- dock panels at every window size -----

fn dock(g: &Gui, area: &str) -> egui::Rect {
    g.rect_of(&format!("panel:area:{area}")).unwrap_or_else(|| panic!("the {area} dock is on screen; docks: {:?}", g.rects("panel:area:")))
}

/// The published rectangle is the dock without its one-point edge line.
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.5
}

fn near3(a: (f32, f32, f32), b: (f32, f32, f32)) -> bool {
    near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2)
}

fn show_every_panel(g: &mut Gui) {
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    g.frames(3);
}

/// What must hold at any window the app allows (960 x 600 points and up): every dock with a panel
/// in it is on screen and big enough to use, and the drawing area keeps its room.
fn docks_are_usable(g: &Gui, what: &str) -> Vec<String> {
    use crate::dock_layout::{CANVAS_MIN_WIDTH, GROUP_MIN_HEIGHT};
    let screen = g.screen();
    let mut bad = Vec::new();
    let shown = |area| crate::panels::PANELS.iter().any(|p| g.app().prefs.panel(p.id).visible && g.app().prefs.panel(p.id).area == area);
    for area in crate::panels::Area::ALL {
        if !shown(area) {
            continue;
        }
        let name = format!("{area:?}");
        let Some(r) = g.rect_of(&format!("panel:area:{name}")) else {
            bad.push(format!("{what}: the {name} dock is missing"));
            continue;
        };
        let (min_w, min_h) = if name == "Bottom" { (screen.width() - 1.0, 99.0) } else { (179.0, GROUP_MIN_HEIGHT - 1.0) };
        if r.width() < min_w || r.height() < min_h || crate::ui_harness::outside(screen, r) {
            bad.push(format!("{what}: the {name} dock is {:.0} x {:.0} at {:?} (window {:?})", r.width(), r.height(), r.min, screen.size()));
        }
        if !g.rects("panel:tab:").iter().any(|(_, t)| r.contains(t.center())) {
            bad.push(format!("{what}: the {name} dock shows no tab"));
        }
    }
    match g.rect_of("canvas") {
        Some(c) if c.width() >= CANVAS_MIN_WIDTH - 40.0 && c.height() >= 150.0 => {}
        c => bad.push(format!("{what}: the drawing area is {:?} (window {:?})", c.map(|c| c.size()), screen.size())),
    }
    bad
}

/// The docks open at their default sizes and stay there: they do not creep down to their
/// narrowest while the app runs (they did: 380 points wide became 334, then 299).
#[test]
fn docks_keep_their_default_size() {
    let mut g = Gui::standard();
    show_every_panel(&mut g);
    g.frames(30);
    assert!(near(dock(&g, "RightTop").width(), 380.0), "{}", dock(&g, "RightTop").width());
    assert!(near(dock(&g, "RightBottom").width(), 380.0), "{}", dock(&g, "RightBottom").width());
    assert!(near(dock(&g, "Left").width(), 260.0), "{}", dock(&g, "Left").width());
    assert!(near(dock(&g, "Bottom").height(), 180.0), "{}", dock(&g, "Bottom").height());
    // Whatever panel is in front: a narrow one does not shrink the dock, a wide one does not widen it.
    for p in crate::panels::PANELS {
        let area = g.app().prefs.panel(p.id).area;
        g.app().prefs.active_tab.insert(area, p.id.to_string());
        g.frames(8);
        assert!(near(dock(&g, "RightTop").width(), 380.0), "with {} in front", p.id);
        assert!(near(dock(&g, "Left").width(), 260.0), "with {} in front", p.id);
        assert!(near(dock(&g, "Bottom").height(), 180.0), "with {} in front", p.id);
    }
}

/// Dragging a dock's edge resizes it, the size stays when the pointer lets go (it sprang back
/// ten points a frame), it is saved with the settings, and a layout reset puts the default back.
#[test]
fn a_dragged_dock_keeps_its_new_size() {
    let mut g = Gui::standard();
    show_every_panel(&mut g);
    let r = dock(&g, "RightTop");
    let edge = egui::pos2(r.left(), r.center().y);
    g.drag(edge, edge - egui::vec2(120.0, 0.0));
    let wide = dock(&g, "RightTop").width();
    assert!((wide - 500.0).abs() < 4.0, "dragged 120 points wider than 380: {wide}");
    g.frames(40);
    assert_eq!(dock(&g, "RightTop").width(), wide, "the size stays");
    assert_eq!(dock(&g, "RightBottom").width(), wide);
    // The bottom dock, dragged taller; the left dock, dragged narrower than it may be.
    let b = dock(&g, "Bottom");
    let top = egui::pos2(b.center().x, b.top());
    g.drag(top, top - egui::vec2(0.0, 100.0));
    let tall = dock(&g, "Bottom").height();
    assert!((tall - 280.0).abs() < 4.0, "{tall}");
    let l = dock(&g, "Left");
    let right = egui::pos2(l.right(), l.center().y);
    g.drag(right, right - egui::vec2(200.0, 0.0));
    g.frames(10);
    assert!(near(dock(&g, "Left").width(), 180.0), "the left dock stops at its narrowest");
    assert_eq!(dock(&g, "Bottom").height(), tall);
    // Saved with the settings.
    let saved = crate::prefs::Prefs::from_json(&g.app().prefs.to_json());
    assert_eq!(saved.dock_sizes, g.app().prefs.dock_sizes);
    assert!(near(saved.dock_sizes["right"], wide) && near(saved.dock_sizes["bottom"], tall), "{:?}", saved.dock_sizes);
    // A click or a drag somewhere else changes no dock.
    g.click_mm(100.0, 100.0);
    g.drag_mm((50.0, 50.0), (90.0, 80.0));
    assert_eq!(dock(&g, "RightTop").width(), wide);
    g.run_command("window.reset_layout");
    show_every_panel(&mut g);
    assert!(near(dock(&g, "RightTop").width(), 380.0), "{}", dock(&g, "RightTop").width());
    assert!(near(dock(&g, "Bottom").height(), 180.0), "{}", dock(&g, "Bottom").height());
}

/// Dock panels went missing at some window sizes: with docks at their real sizes, a small window
/// left the side docks a strip under the tab row, or the drawing area nothing. At every size from
/// the smallest window up, started at that size, every dock and the drawing area are usable.
#[test]
fn every_dock_is_usable_at_every_window_size() {
    let mut bad = Vec::new();
    for (w, h) in
        [(960.0, 600.0), (1024.0, 600.0), (960.0, 1400.0), (1100.0, 700.0), (1280.0, 720.0), (1600.0, 1000.0), (2560.0, 700.0), (3840.0, 2100.0)]
    {
        let mut g = Gui::new(w, h, 1.0);
        bad.extend(docks_are_usable(&g, &format!("default panels, {w} x {h}")));
        show_every_panel(&mut g);
        bad.extend(docks_are_usable(&g, &format!("every panel, {w} x {h}")));
        bad.extend(g.layout_problems().into_iter().map(|p| format!("every panel, {w} x {h}: {p}")));
        g.screenshot(&format!("uibugs-docks-{w}x{h}"));
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

/// The user's widest docks in a big window, then the window made small: the docks give way, the
/// drawing area stays, and back at the big size the docks have the user's sizes again.
#[test]
fn docks_give_way_when_the_window_shrinks_and_come_back_when_it_grows() {
    let mut g = Gui::new(2560.0, 1400.0, 1.0);
    show_every_panel(&mut g);
    for (key, size) in [("left", 520.0), ("right", 640.0), ("bottom", 480.0)] {
        g.app().prefs.dock_sizes.insert(key.to_string(), size);
    }
    g.frames(3);
    let big = |g: &Gui| (dock(g, "Left").width().round(), dock(g, "RightTop").width().round(), dock(g, "Bottom").height().round());
    assert!(near3(big(&g), (520.0, 640.0, 480.0)), "{:?}", big(&g));
    let mut bad = Vec::new();
    for (w, h) in [(1600.0, 1000.0), (1280.0, 720.0), (960.0, 600.0), (1280.0, 720.0), (960.0, 1400.0), (2560.0, 600.0), (960.0, 600.0)] {
        g.set_window_size(w, h);
        bad.extend(docks_are_usable(&g, &format!("shrunk to {w} x {h}")));
        bad.extend(g.layout_problems().into_iter().map(|p| format!("shrunk to {w} x {h}: {p}")));
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
    g.screenshot("uibugs-docks-shrunk-960x600");
    g.set_window_size(2560.0, 1400.0);
    assert!(near3(big(&g), (520.0, 640.0, 480.0)), "the chosen sizes come back with the room: {:?}", big(&g));
}

/// A window far below the smallest the app allows (a forced size, or a frame drawn while the
/// window is being minimised) draws without a panic, and the layout is whole again afterwards.
#[test]
fn a_tiny_window_draws_and_the_layout_recovers() {
    let mut g = Gui::standard();
    g.allow_id_clashes();
    show_every_panel(&mut g);
    for (w, h) in [(640.0, 400.0), (300.0, 200.0), (400.0, 1000.0), (1600.0, 120.0), (40.0, 30.0), (2.0, 2.0), (700.0, 500.0)] {
        g.set_window_size(w, h);
        g.frames(3);
    }
    g.set_window_size(1600.0, 1000.0);
    let bad = docks_are_usable(&g, "back at 1600 x 1000");
    assert!(bad.is_empty(), "{bad:?}");
    assert!(near(dock(&g, "RightTop").width(), 380.0), "{}", dock(&g, "RightTop").width());
    assert!(near(dock(&g, "Left").width(), 260.0), "{}", dock(&g, "Left").width());
    assert!(near(dock(&g, "Bottom").height(), 180.0), "{}", dock(&g, "Bottom").height());
    assert!(g.id_clashes_now().is_empty());
}

// ----- dialogs with a machine connected -----

/// Machine Settings with the settings of a connected (simulated) GRBL machine listed: the window
/// fits the smallest window. It was 1317 points wide in a 960 point window, its left part and
/// the Close button's row off screen.
#[test]
fn machine_settings_with_settings_listed_fits_the_smallest_window() {
    for (w, h) in [(960.0, 600.0), (1280.0, 720.0), (1920.0, 1080.0)] {
        let mut g = Gui::new(w, h, 1.0);
        g.run_command("machine.connect");
        g.run_command("machine.read_settings");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while g.app().machine.handle.snapshot().grbl_settings.is_empty() {
            assert!(std::time::Instant::now() < end, "the simulator answered with its settings");
            std::thread::sleep(std::time::Duration::from_millis(10));
            g.frames(2);
        }
        g.run_command("edit.machine_settings");
        g.frames(3);
        g.screenshot(&format!("uibugs-machine-settings-{w}x{h}"));
        assert!(g.has_text("ALL SETTINGS") && g.window_titles().contains(&"Machine Settings".to_string()), "{:?}", g.visible_texts());
        let screen = g.screen();
        for r in g.dialog_windows() {
            assert!(!crate::ui_harness::outside(screen, r), "{w} x {h}: the window {r:?} is not inside {screen:?}");
        }
        let problems = g.layout_problems();
        assert!(problems.is_empty(), "{w} x {h}: {problems:#?}");
        g.close_dialogs();
    }
}
/// With the simulator connected and its settings read (the sweeps in `ui_smoke_tests` run with
/// nothing connected), at the smallest window: every window a command opens and every panel shows
/// no clipped, broken or off-screen text, and every window is inside the screen.
#[test]
fn nothing_clips_in_any_window_or_panel_with_a_machine_connected() {
    let mut g = Gui::new(960.0, 600.0, 1.0);
    g.run_command("machine.connect");
    g.run_command("machine.read_settings");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while g.app().machine.handle.snapshot().grbl_settings.is_empty() {
        assert!(std::time::Instant::now() < end, "the simulator answered with its settings");
        std::thread::sleep(std::time::Duration::from_millis(10));
        g.frames(2);
    }
    let mut bad = std::collections::BTreeSet::new();
    let ids: Vec<&str> = commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    for id in ids {
        // Commands that act on the machine or leave the app are not window openers.
        if id.starts_with("machine.") || id == "file.exit" {
            continue;
        }
        g.close_dialogs();
        crate::files::new_project(&mut g.app());
        g.sample_design();
        g.run_command(id);
        g.frames(6);
        if g.dialog_open() {
            for p in g.layout_problems() {
                bad.insert(format!("{id} {:?}: {p}", g.window_titles()));
            }
        }
    }
    g.close_dialogs();
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    for p in crate::panels::PANELS {
        let area = g.app().prefs.panel(p.id).area;
        g.app().prefs.active_tab.insert(area, p.id.to_string());
        g.frames(3);
        for q in g.layout_problems() {
            bad.insert(format!("panel {}: {q}", p.id));
        }
    }
    assert!(g.app().machine.handle.is_connected(), "the simulator stayed connected");
    assert!(bad.is_empty(), "{} layout problems:\n{}", bad.len(), bad.into_iter().collect::<Vec<_>>().join("\n"));
}
