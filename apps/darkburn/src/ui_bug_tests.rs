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

// ----- the Preview window -----

const PREVIEW_CHECKS: [&str; 3] = ["Shade by power", "Show travel moves", "Invert"];

/// The Preview window with the job of the sample design in it.
fn open_preview(g: &mut Gui) {
    g.sample_design();
    g.run_command("window.preview");
    g.wait_for_job_build();
    g.frames(4);
    assert!(g.app().job.preview.data.is_some(), "the preview has a job: {:?}", g.app().job.preview.error);
}

fn preview_window(g: &Gui) -> egui::Rect {
    g.find_a11y(Some("Window"), "Preview").last().unwrap_or_else(|| panic!("the Preview window is open: {:?}", g.window_titles())).rect
}

fn preview_check(g: &Gui, label: &str) -> Option<bool> {
    let view = g.app().job.preview.view;
    [view.shade_by_power, view.show_travel, view.invert].get(PREVIEW_CHECKS.iter().position(|l| *l == label)?).copied()
}

/// What is wrong with the Preview's three check boxes: each must be drawn inside the window, as
/// wide as its box, the gap and all of its label, with the label painted whole (to half a point:
/// the label starts on a whole pixel). A box squeezed into the end of a row keeps its name for a
/// screen reader and for `find_text`, but shows "...".
fn preview_check_problems(g: &Gui, what: &str) -> Vec<String> {
    let t = solidcraft_theme::Tokens::get();
    let window = preview_window(g);
    let painted = g.painted();
    let mut bad = Vec::new();
    for label in PREVIEW_CHECKS {
        let font = egui::FontId::proportional(t.font_label);
        let words = g.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, egui::Color32::WHITE)).size().x;
        let rect = g.find_a11y(Some("CheckBox"), label).last().map(|n| n.rect);
        let text = painted.iter().rev().find(|p| p.text == label);
        match (rect, text) {
            (Some(r), Some(p))
                if r.width() >= t.space_14 + t.space_6 + words - 0.5
                    && r.height() > 0.0
                    && window.contains_rect(r)
                    && p.full.width() >= words - 0.5
                    && p.rect.expand(0.5).contains_rect(p.full)
                    && window.contains_rect(p.full) => {}
            _ => bad.push(format!(
                "{what}: the check box {label:?} is at {rect:?}, its label ({words} points wide) is painted as {text:?}, the window is {window:?}"
            )),
        }
    }
    bad
}

/// The Preview of one filled square, 100 mm a side, on a layer at `speed` mm/s: an hour and more
/// of scanning at 20 mm/s. Returns how long the job takes, in seconds.
fn open_preview_of_a_fill(g: &mut Gui, speed: f64) -> f64 {
    use serde_json::json;
    if g.app().editor.doc.used_layers().is_empty() {
        g.exec("shape.add_rect", json!({ "x": 20, "y": 20, "width": 100, "height": 100, "layer": 1 }));
        g.exec("layer.set_mode", json!({ "layer": 1, "mode": "fill" }));
    }
    g.exec("layer.set_param", json!({ "layer": 1, "field": "speed", "value": speed }));
    g.run_command("window.preview");
    g.wait_for_job_build();
    g.frames(4);
    let total = g.app().job.preview.data.as_ref().map(|d| d.stats.seconds);
    total.unwrap_or_else(|| panic!("the preview has a job: {:?}", g.app().job.preview.error))
}

fn size_of(r: egui::Rect) -> (f32, f32) {
    (r.width(), r.height())
}

/// The Preview window keeps the size it opens at. It got 18 points wider with every frame, until
/// it was as wide as the main window.
#[test]
fn the_preview_window_keeps_its_size() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let first = preview_window(&g);
    g.frames(60);
    let later = preview_window(&g);
    g.screenshot("uibugs-preview");
    assert_eq!((first.width(), first.height()), (later.width(), later.height()), "60 frames later the window is {later:?}; it was {first:?}");
    // And that size is the one it asks for, 760 by 600 points, 80 points in and 60 down. (It was a
    // point or three off in height: the room for the buttons was not what they took.)
    assert_eq!((later.min, size_of(later)), (egui::pos2(80.0, 60.0), (760.0, 600.0)), "the Preview opens as {later:?}");
    let screen = g.screen();
    assert!(later.width() < screen.width() - 200.0, "the Preview {later:?} is narrower than the main window {screen:?}");
    assert!(!crate::ui_harness::outside(screen, later), "the Preview {later:?} is inside the main window {screen:?}");
    let problems = g.layout_problems();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Shade by power, Show travel moves and Invert are on screen, and a click on each flips it.
/// They were drawn zero points wide beside the speed list, which took the whole row.
#[test]
fn the_preview_check_boxes_can_be_seen_and_clicked() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let problems = preview_check_problems(&g, "as opened");
    assert!(problems.is_empty(), "{problems:#?}");
    for label in PREVIEW_CHECKS {
        let before = preview_check(&g, label).expect("a check box of the Preview");
        g.click_a11y(Some("CheckBox"), label);
        assert_eq!(preview_check(&g, label), Some(!before), "a click on {label:?} flips it");
        g.click_label(label);
        assert_eq!(preview_check(&g, label), Some(before), "a click on the words {label:?} flips it back");
    }
    // The speed list is still there, beside them, and no wider than it needs to be.
    let speed = g.find_a11y(Some("ComboBox"), "10x").last().map(|n| n.rect).expect("the speed list");
    assert!(speed.width() > 40.0 && speed.width() < 200.0, "the speed list is {speed:?}");
    g.click_pos(speed.center());
    g.click_label("500x");
    assert_eq!(g.app().job.preview.speed_index, crate::job_cmds::SPEEDS.len() - 1);
    let problems = preview_check_problems(&g, "after picking a speed");
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Made as narrow as it goes, the Preview still shows its check boxes (on a row of their own if
/// they must), and it does not grow back, wider or taller, while it is left alone.
#[test]
fn a_narrow_preview_window_keeps_its_size_and_its_check_boxes() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let wide = preview_window(&g);
    // Drag the right edge far to the left: the window stops at the narrowest its rows allow.
    let grip = egui::pos2(wide.right() - 1.0, wide.center().y);
    g.drag(grip, egui::pos2(wide.left() + 40.0, wide.center().y));
    g.frames(4);
    let narrow = preview_window(&g);
    assert!(narrow.width() < wide.width() - 100.0, "the window got narrower: {narrow:?} from {wide:?}");
    g.frames(60);
    let later = preview_window(&g);
    assert_eq!((narrow.width(), narrow.height()), (later.width(), later.height()), "60 frames later the window is {later:?}; it was {narrow:?}");
    let mut problems = preview_check_problems(&g, "narrow");
    problems.extend(g.layout_problems());
    g.screenshot("uibugs-preview-narrow");
    assert!(problems.is_empty(), "{problems:#?}");
    for label in PREVIEW_CHECKS {
        let before = preview_check(&g, label).expect("a check box of the Preview");
        g.click_a11y(Some("CheckBox"), label);
        assert_eq!(preview_check(&g, label), Some(!before), "narrow: a click on {label:?} flips it");
    }
}

/// A job of an hour or more has a longer clock, "1h 07m 51s / 1h 07m 51s". The row with the time
/// slider gave the clock 144 points whatever it said, so with such a job the window got 7 points
/// wider with every frame (23 with a job of ten hours), until it was wider than the main window.
#[test]
fn the_preview_window_keeps_its_size_with_a_job_of_hours() {
    let mut g = Gui::standard();
    let screen = g.screen();
    for (speed, at_least) in [(20.0, 3600.0), (2.0, 36_000.0), (0.05, 360_000.0)] {
        let total = open_preview_of_a_fill(&mut g, speed);
        assert!(total >= at_least, "at {speed} mm/s the job takes {total:.0} s");
        let first = preview_window(&g);
        g.frames(60);
        let later = preview_window(&g);
        assert_eq!(size_of(first), size_of(later), "{total:.0} s: 60 frames later the window is {later:?}; it was {first:?}");
        assert!(later.width() < screen.width() - 200.0, "{total:.0} s: the Preview {later:?} is narrower than the main window {screen:?}");
        // The clock is there, whole, at the end of the job and anywhere in it.
        for part in [1.0, 0.0, 0.37, 0.5, 0.999] {
            g.app().job.preview.t = total * part;
            g.frames(10);
            let window = preview_window(&g);
            assert_eq!(size_of(window), size_of(first), "{total:.0} s, clock at {part}: the window is {window:?}; it was {first:?}");
            let clock = format!("{} / {}", crate::job_build::format_time(total * part), crate::job_build::format_time(total));
            let painted = g.painted().into_iter().rev().find(|p| p.text == clock).unwrap_or_else(|| panic!("the clock {clock:?} shows"));
            assert!(painted.rect == painted.full && window.contains_rect(painted.full), "the clock {painted:?} is whole inside {window:?}");
            let slider = g.find_a11y(Some("Slider"), "Time in the job").last().expect("the time slider").rect;
            assert!(slider.width() >= 80.0 && slider.right() <= painted.full.left(), "the slider {slider:?} ends before the clock {painted:?}");
        }
        let problems = g.layout_problems();
        assert!(problems.is_empty(), "{total:.0} s: {problems:#?}");
    }
    g.screenshot("uibugs-preview-long-job");
}

/// The bottom right corner follows the pointer that drags it: the window grows by what the
/// pointer moved. (It grew by 16 points less in height, then by 2 less: the picture took a
/// little less than the room the buttons leave, so the window was shorter than its own size.)
#[test]
fn the_preview_window_grows_by_what_its_corner_is_dragged() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let before = preview_window(&g);
    let corner = before.max - egui::vec2(3.0, 3.0);
    g.drag(corner, corner + egui::vec2(60.0, 40.0));
    let after = preview_window(&g);
    assert_eq!(after.min, before.min, "the top left corner stays");
    assert_eq!(size_of(after), (before.width() + 60.0, before.height() + 40.0), "dragged by (60, 40): {after:?} from {before:?}");
    g.frames(30);
    assert_eq!(preview_window(&g), after, "and it stays that size");
    // Smaller again, by the same corner.
    let corner = after.max - egui::vec2(3.0, 3.0);
    g.drag(corner, corner - egui::vec2(100.0, 90.0));
    let small = preview_window(&g);
    assert_eq!(size_of(small), (after.width() - 100.0, after.height() - 90.0), "dragged by (-100, -90): {small:?} from {after:?}");
}

/// Taking hold of an edge does not change the size the pointer did not ask for. Every grab of
/// the right edge made the window 2 points shorter (16 before that).
#[test]
fn taking_hold_of_the_previews_edge_does_not_make_it_shorter() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    let start = preview_window(&g);
    for grab in 0..10 {
        let r = preview_window(&g);
        let grip = egui::pos2(r.right() - 1.0, r.center().y);
        let dx = if grab % 2 == 0 { 6.0 } else { -6.0 };
        g.drag(grip, grip + egui::vec2(dx, 0.0));
        let now = preview_window(&g);
        assert_eq!(now.height(), start.height(), "grab {grab}: the window is {now:?}; it started as {start:?}");
        assert_eq!(now.width(), r.width() + dx, "grab {grab}: the right edge followed the pointer");
    }
    assert_eq!(preview_window(&g), start, "ten grabs later the window is as it was");
    // The bottom edge too: the width stays.
    for grab in 0..4 {
        let r = preview_window(&g);
        let grip = egui::pos2(r.center().x, r.bottom() - 1.0);
        let dy = if grab % 2 == 0 { 10.0 } else { -10.0 };
        g.drag(grip, grip + egui::vec2(0.0, dy));
        let now = preview_window(&g);
        assert_eq!(size_of(now), (start.width(), r.height() + dy), "bottom edge, grab {grab}: {now:?} from {r:?}");
    }
}

/// How many rows the Preview's buttons under the picture are on.
fn preview_button_rows(g: &Gui) -> usize {
    let mut tops: Vec<i32> = ["Start here", "Run from here", "Send from here", "Save from here", "Save image", "Fit", "Maximise", "Refresh", "Close"]
        .iter()
        .filter_map(|label| g.find_a11y(Some("Button"), label).last().map(|n| n.rect.top().round() as i32))
        .collect();
    tops.sort_unstable();
    tops.dedup();
    tops.len()
}

/// Made narrower a little at a time, the Preview keeps its height while its buttons go to two,
/// three and four rows, and has it still when it is wide again. (The picture left the buttons
/// the room they took in the frame before, so a new row could make the window 20 points taller,
/// and it stayed that tall.)
#[test]
fn the_preview_keeps_its_height_when_its_buttons_go_to_more_rows() {
    // The pointer moves 6 points in one, two or three frames before it lets go: which frame a
    // row of buttons wraps in decides what a height from the frame before does to the window.
    for moves in [1, 2, 3] {
        let mut g = Gui::standard();
        open_preview(&mut g);
        let start = preview_window(&g);
        assert_eq!(preview_button_rows(&g), 1, "at {start:?} the buttons are on one row");
        let mut most = 1;
        let mut step = |g: &mut Gui, dx: f32| {
            let r = preview_window(g);
            let grip = egui::pos2(r.right() - 1.0, r.center().y);
            g.drag_steps(grip, grip + egui::vec2(dx, 0.0), moves);
            let now = preview_window(g);
            let rows = preview_button_rows(g);
            assert_eq!(
                now.height(),
                start.height(),
                "{moves} moves a drag: at {:.0} points wide, {rows} rows of buttons, the window is {now:?}; it started as {start:?}",
                now.width()
            );
            most = most.max(rows);
            now.width() != r.width()
        };
        // Narrower until it stops at its narrowest, then wide again.
        for _ in 0..200 {
            if !step(&mut g, -6.0) {
                break;
            }
        }
        let narrow = preview_window(&g);
        assert!(narrow.width() < 320.0, "the window went down to its narrowest: {narrow:?}");
        for _ in 0..200 {
            if preview_window(&g).width() >= start.width() {
                break;
            }
            step(&mut g, 6.0);
        }
        assert!(most >= 4, "the buttons went to {most} rows on the way");
        assert_eq!(preview_button_rows(&g), 1);
        let wide = preview_window(&g);
        assert!(wide.width() >= start.width(), "{wide:?}");
        assert_eq!(wide.height(), start.height(), "wide again, the window is {wide:?}; it started as {start:?}");
        let mut problems = preview_check_problems(&g, "wide again");
        problems.extend(g.layout_problems());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

/// Maximise fills the room under the application bar; Restore puts the window back where it was,
/// at the size it had. (Restore left it as big as the main window, and it lay over the bar and
/// the main window's own minimise, maximise and close buttons.)
#[test]
fn restore_puts_the_preview_back_where_it_was() {
    let mut g = Gui::standard();
    open_preview(&mut g);
    // Not where and how it opens: moved and sized first.
    let r = preview_window(&g);
    let grip = egui::pos2(r.center().x, r.top() + 14.0);
    g.drag(grip, grip + egui::vec2(140.0, 90.0));
    let r = preview_window(&g);
    let corner = r.max - egui::vec2(3.0, 3.0);
    g.drag(corner, corner + egui::vec2(-70.0, 50.0));
    let before = preview_window(&g);
    assert_eq!((before.min, size_of(before)), (r.min, (r.width() - 70.0, r.height() + 50.0)));
    let screen = g.screen();
    let bar = crate::shell::APP_BAR_HEIGHT;
    for round in 0..2 {
        g.click_a11y(Some("Button"), "Maximise");
        g.frames(4);
        assert!(g.app().job.preview.maximised);
        let big = preview_window(&g);
        assert!(big.top() >= bar && !crate::ui_harness::outside(screen, big), "round {round}: maximised, {big:?} is under the application bar");
        assert!(
            big.width() > screen.width() - 40.0 && big.height() > screen.height() - bar - 40.0,
            "round {round}: maximised, {big:?} fills the room under the bar"
        );
        g.frames(30);
        assert_eq!(preview_window(&g), big, "round {round}: the maximised window is steady");
        let mut problems = preview_check_problems(&g, "maximised");
        problems.extend(g.layout_problems());
        assert!(problems.is_empty(), "round {round}: {problems:#?}");
        assert_eq!(g.window_titles(), ["Preview"], "one Preview window");
        g.click_a11y(Some("Button"), "Restore");
        g.frames(4);
        assert!(!g.app().job.preview.maximised);
        assert_eq!(preview_window(&g), before, "round {round}: Restore puts the window back");
        assert!(!g.find_a11y(Some("Button"), "Maximise").is_empty());
    }
    // Escape closes it from either state, and it comes back as it was.
    g.click_a11y(Some("Button"), "Maximise");
    g.frames(4);
    g.escape();
    assert!(!g.app().job.preview.open && !g.dialog_open());
}

/// The Preview opens under the application bar, whole, also in the smallest main window and
/// with the interface scaled up in it. (In a 960 x 600 window it opened at the very top, over
/// the bar; at 200 % its last row of buttons was below the bottom of the screen.)
#[test]
fn the_preview_opens_under_the_application_bar_in_any_window() {
    let bar = crate::shell::APP_BAR_HEIGHT;
    for (w, h, scale) in [(1600.0, 1000.0, 1.0), (1440.0, 960.0, 2.0), (960.0, 600.0, 1.0), (640.0, 400.0, 1.5), (480.0, 300.0, 2.0)] {
        let mut g = Gui::new(w, h, scale);
        g.allow_id_clashes();
        open_preview(&mut g);
        let screen = g.screen();
        let window = preview_window(&g);
        let what = format!("{w} x {h} points at scale {scale}");
        assert!(window.top() >= bar - 0.5, "{what}: the Preview {window:?} opens under the application bar");
        assert!(!crate::ui_harness::outside(screen, window), "{what}: the Preview {window:?} is inside {screen:?}");
        for label in ["Start here", "Save image", "Fit", "Maximise", "Refresh", "Close"] {
            let b = g.find_a11y(Some("Button"), label).last().unwrap_or_else(|| panic!("{what}: the button {label}")).rect;
            assert!(window.contains_rect(b) && screen.contains_rect(b), "{what}: the button {label} at {b:?} is inside {window:?}");
        }
        g.frames(30);
        assert_eq!(preview_window(&g), window, "{what}: the window is steady");
        let problems = preview_check_problems(&g, &what);
        assert!(problems.is_empty(), "{problems:#?}");
        // Maximised it is under the bar too, and Restore still puts it back.
        g.click_a11y(Some("Button"), "Maximise");
        g.frames(4);
        let big = preview_window(&g);
        assert!(big.top() >= bar - 0.5 && !crate::ui_harness::outside(screen, big), "{what}: maximised, the Preview is at {big:?}");
        g.click_a11y(Some("Button"), "Restore");
        g.frames(4);
        assert_eq!(preview_window(&g), window, "{what}: Restore puts the window back");
    }
}

// ----- the Cut Settings Editor -----

/// The Cut Settings Editor on a layer that fills, in a window of this size.
fn open_cut_settings(width: f32, height: f32) -> Gui {
    let mut g = Gui::new(width, height, 1.0);
    g.sample_design();
    g.exec("layer.set_mode", serde_json::json!({ "layer": 1, "mode": "fill" }));
    g.app().cut_settings_open = Some(1);
    g.frames(6);
    assert_eq!(g.window_titles(), ["Cut Settings Editor"]);
    g
}

/// Opening another project, or making a new one, while the Cut Settings Editor is open closes
/// the editor and leaves the new project as it came. Cancel (or the tab's X) used to write the
/// old project's layers into the new one, and closing it from elsewhere left an undo step there.
#[test]
fn the_cut_settings_editor_closes_when_another_project_takes_the_place_of_its_own() {
    for closed_from_elsewhere in [false, true] {
        let mut g = open_cut_settings(1440.0, 960.0);
        g.app().editor.doc.layer_mut(1).main_mut().speed = 123.0;
        g.frames(2);
        crate::files::new_project(&mut g.app());
        let (layers, steps, modified) = {
            let app = g.app();
            (app.editor.doc.layers.clone(), app.editor.history.len(), app.editor.is_modified())
        };
        if closed_from_elsewhere {
            g.app().cut_settings_open = None;
        }
        g.frames(3);
        assert_eq!(g.app().cut_settings_open, None, "closed from elsewhere: {closed_from_elsewhere}");
        assert!(!g.window_titles().iter().any(|t| t == "Cut Settings Editor"), "{:?}", g.window_titles());
        let app = g.app();
        assert!(app.editor.doc.layers == layers, "the new project keeps its own layers");
        assert_eq!((app.editor.history.len(), app.editor.is_modified()), (steps, modified), "and gets no undo step");
    }
}

/// The hint under the layer list is whole. It was cut off after "severa", on one row.
#[test]
fn the_hint_under_the_cut_settings_layer_list_is_not_cut_off() {
    let g = open_cut_settings(1440.0, 960.0);
    let hint = g.painted().into_iter().find(|p| p.text.starts_with("Ctrl+click")).expect("the hint under the layer list");
    let layers = g.find_text("Layers").expect("the heading of the layer list");
    assert!(hint.text.ends_with("layers at once."), "{:?}", hint.text);
    assert!(hint.full.height() > 20.0, "the hint wraps to a second row: it is {:?}", hint.full);
    assert!(hint.full.width() <= 130.0 && hint.full.left() >= layers.left() - 1.0, "the hint stays in the list's column: {:?}", hint.full);
    assert_eq!(hint.rect, hint.full, "all of the hint shows");
    assert!(!hint.broken_word, "no word of the hint is broken");
    let problems = g.layout_problems();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The fields of the Cut Settings Editor go down to the bottom of the window, so the rows of a
/// fill layer down to Z offset show without scrolling in a window with the room. (They stopped
/// 460 points down, with the lower third of the window empty.) And the window is no taller than
/// its two columns, so it fits under the application bar, where it opens.
#[test]
fn the_cut_settings_fields_go_down_to_the_bottom_of_the_window() {
    let g = open_cut_settings(1440.0, 960.0);
    let window = *g.dialog_windows().last().expect("the Cut Settings Editor");
    for label in ["Bi-directional", "Cross-hatch", "Overscan", "Z offset", "Z step per pass"] {
        let r = g.find_text(label).unwrap_or_else(|| panic!("{label} shows without scrolling; texts: {:?}", g.visible_texts()));
        assert!(window.contains_rect(r), "{label} at {r:?} is inside the window {window:?}");
    }
    // Under the last row that shows there is the OK and Cancel row, not a third of a window.
    let last = g.find_text("Z step per pass").expect("the last row").bottom();
    let ok = g.find_text("OK").expect("the OK button").top();
    assert!(ok - last < 140.0, "{:.0} points lie between the last row ({last}) and the OK button ({ok})", ok - last);
    let room = g.screen().height() - crate::shell::APP_BAR_HEIGHT;
    assert!(window.height() <= room - 12.0, "the window ({:.0} points tall) fits under the application bar ({room:.0} points)", window.height());
    assert!(window.top() >= crate::shell::APP_BAR_HEIGHT, "the window {window:?} is under the application bar");
    let problems = g.layout_problems();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The Cut Settings Editor opens in the middle of the room under the application bar, in a big
/// main window and in the smallest. (It opened 70 points above the middle of the main window:
/// at 1440 x 960 its top was at the very top, over the bar and the menus from Laser Tools to
/// Help, with 126 points free below it.)
#[test]
fn the_cut_settings_editor_opens_under_the_application_bar() {
    let bar = crate::shell::APP_BAR_HEIGHT;
    for (w, h) in [(1440.0, 960.0), (1440.0, 880.0), (1600.0, 1000.0), (2560.0, 1400.0), (1280.0, 720.0), (960.0, 600.0)] {
        let mut g = open_cut_settings(w, h);
        let screen = g.screen();
        let window = *g.dialog_windows().last().expect("the Cut Settings Editor");
        assert!(window.top() >= bar && window.bottom() <= screen.bottom(), "{w} x {h}: the editor {window:?} is under the application bar");
        let (above, below) = (window.top() - bar, screen.bottom() - window.bottom());
        assert!((above - below).abs() <= 1.5, "{w} x {h}: {above} points above the editor {window:?} and {below} below it");
        assert!((window.center().x - screen.center().x).abs() <= 1.5, "{w} x {h}: the editor {window:?} is in the middle");
        g.frames(30);
        assert_eq!(*g.dialog_windows().last().expect("the editor"), window, "{w} x {h}: the editor stays where it opened");
        // The other tab is as tall: the window does not jump.
        g.click_label("Advanced");
        g.frames(4);
        assert_eq!(*g.dialog_windows().last().expect("the editor"), window, "{w} x {h}: the Advanced tab is in the same window");
        let problems = g.layout_problems();
        assert!(problems.is_empty(), "{w} x {h}: {problems:#?}");
    }
}

/// In the smallest window the editor still fits, and its fields scroll.
#[test]
fn the_cut_settings_editor_fits_the_smallest_window() {
    let mut g = open_cut_settings(960.0, 600.0);
    let screen = g.screen();
    let window = *g.dialog_windows().last().expect("the Cut Settings Editor");
    assert!(!crate::ui_harness::outside(screen, window), "the window {window:?} is inside {screen:?}");
    let problems = g.layout_problems();
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(!g.has_text("Z offset"), "the last rows are below the fold here");
    let at = g.find_text("Speed").expect("a row of the fields").center();
    for _ in 0..12 {
        if g.has_text("Z step per pass") {
            break;
        }
        g.scroll(at, egui::vec2(0.0, -200.0));
    }
    let r = g.find_text("Z step per pass").expect("the fields scroll down to the last row");
    assert!(window.contains_rect(r), "{r:?} is inside {window:?}");
    assert_eq!(*g.dialog_windows().last().expect("the editor"), window, "scrolling the fields does not move or resize the window");
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

// ----- Adjust Image and Trace Image -----

/// An app with a small grey picture on the page, selected.
fn gui_with_a_picture(width: f32, height: f32) -> Gui {
    let mut g = Gui::new(width, height, 1.0);
    let file = g.temp("ramp.png");
    image::GrayImage::from_fn(90, 60, |x, y| image::Luma([((x * 2 + y * 3) % 256) as u8])).save(&file).expect("write the picture");
    let r = g.exec("project.import", serde_json::json!({ "paths": [file.to_string_lossy()], "x": 100, "y": 100 }));
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    g
}

/// The two windows with a column of fields beside a picture, the command that opens each, and
/// the last thing in its column of fields.
const IMAGE_WINDOWS: [(&str, &str, &str); 2] =
    [("tools.adjust_image", "Adjust Image", "Export..."), ("tools.trace_image", "Trace Image", "Delete image after tracing")];

/// Adjust Image and Trace Image are as tall as what is in them: under the last row there is the
/// row of OK and Cancel. (Both were as tall as the main window whatever was in them, with their
/// lower third empty and their top edge across the application bar.)
#[test]
fn adjust_image_and_trace_image_are_no_taller_than_what_is_in_them() {
    for (command, title, last_row) in IMAGE_WINDOWS {
        let mut g = gui_with_a_picture(1440.0, 960.0);
        g.run_command(command);
        g.settle();
        g.frames(4);
        assert_eq!(g.window_titles(), [title]);
        g.screenshot(&format!("uibugs-{}", title.to_lowercase().replace(' ', "-")));
        let (screen, window) = (g.screen(), *g.dialog_windows().last().expect("the window"));
        assert!(g.has_text(last_row), "{title}: {last_row} shows without scrolling: {:?}", g.visible_texts());
        // The column of fields is the taller of the two with this picture: the row of OK and
        // Cancel comes right under its last row.
        let ok = g.find_text("OK").expect("the OK button").top();
        let last = g.find_text(last_row).expect("the last row").bottom();
        assert!(
            ok > last && ok - last < 80.0,
            "{title}: {:.0} points lie between {last_row} ({last}) and the OK button ({ok}) of {window:?}",
            ok - last
        );
        assert!(window.top() >= crate::shell::APP_BAR_HEIGHT, "{title}: the window {window:?} leaves the application bar free");
        assert!(!crate::ui_harness::outside(screen, window), "{title}: the window {window:?} is inside {screen:?}");
        let problems = g.layout_problems();
        assert!(problems.is_empty(), "{title}: {problems:#?}");
        g.frames(20);
        assert_eq!(*g.dialog_windows().last().expect("the window"), window, "{title}: the window stays as it is");
    }
}

/// In a small main window Adjust Image and Trace Image fit under the application bar, and what
/// does not show scrolls.
#[test]
fn adjust_image_and_trace_image_fit_a_small_window_under_the_application_bar() {
    for (command, title, last_row) in IMAGE_WINDOWS {
        for (width, height) in [(960.0, 600.0), (1100.0, 700.0)] {
            let mut g = gui_with_a_picture(width, height);
            g.run_command(command);
            g.settle();
            g.frames(4);
            assert_eq!(g.window_titles(), [title]);
            let (screen, window) = (g.screen(), *g.dialog_windows().last().expect("the window"));
            let at = format!("{title} in {width} x {height}");
            assert!(!crate::ui_harness::outside(screen, window), "{at}: the window {window:?} is inside {screen:?}");
            assert!(window.top() >= crate::shell::APP_BAR_HEIGHT, "{at}: the window {window:?} leaves the application bar free");
            let problems = g.layout_problems();
            assert!(problems.is_empty(), "{at}: {problems:#?}");
            let ok = g.find_text("OK").expect("the OK button");
            assert!(window.contains_rect(ok), "{at}: the OK button {ok:?} is inside {window:?}");
            let over = pos2_in_the_fields(window);
            for _ in 0..12 {
                if g.find_text(last_row).is_some_and(|r| r.bottom() < ok.top()) {
                    break;
                }
                g.scroll(over, egui::vec2(0.0, -200.0));
            }
            let r = g.find_text(last_row).unwrap_or_else(|| panic!("{at}: the fields scroll down to {last_row}: {:?}", g.visible_texts()));
            assert!(window.contains_rect(r) && r.bottom() < ok.top(), "{at}: {last_row} at {r:?} is inside {window:?}, above the OK button");
            assert_eq!(*g.dialog_windows().last().expect("the window"), window, "{at}: scrolling does not move or resize the window");
        }
    }
}

/// A point over the column of fields of an image window: a quarter in from its left edge.
fn pos2_in_the_fields(window: egui::Rect) -> egui::Pos2 {
    egui::pos2(window.left() + window.width() * 0.25, window.center().y)
}
