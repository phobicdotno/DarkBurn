//! Smoke tests that drive the whole UI through `ui_harness`. See docs/UI-TESTING.md.
use crate::commands::{self, Entry};
use crate::ui_harness::Gui;

#[test]
fn the_app_starts_and_draws() {
    let mut g = Gui::standard();
    g.frames(3);
    assert!(g.has_text("File"), "{:?}", g.visible_texts());
    let p = g.screenshot("start");
    assert!(p.exists());
}

// ----- the harness itself -----

#[test]
fn the_harness_clicks_by_label_id_and_accessibility_node() {
    let mut g = Gui::standard();
    g.click_label("Modify");
    assert_eq!(g.app().prefs.ribbon_tab, "modify");
    g.click_id("ribbon:tab:text");
    assert_eq!(g.app().prefs.ribbon_tab, "text");
    g.open_menu("Help", "About DarkBurn");
    assert!(g.app().about_open);
    assert_eq!(g.find_a11y(Some("Window"), "About DarkBurn").len(), 1);
    g.click_a11y(Some("Button"), "Close");
    assert!(!g.app().about_open && !g.dialog_open());
}

#[test]
fn the_harness_drags_double_clicks_and_scrolls_on_the_canvas() {
    let mut g = Gui::standard();
    // Design millimetres and screen points convert both ways.
    let p = g.mm_to_screen(50.0, 70.0);
    let back = g.screen_to_mm(p);
    assert!((back.x - 50.0).abs() < 0.01 && (back.y - 70.0).abs() < 0.01, "{back:?}");
    // A drag with the rectangle tool draws a rectangle; a double click on its layer row (not on the name, which renames) opens the editor.
    g.click_id("tool:tools.rect");
    g.drag_mm((30.0, 30.0), (90.0, 60.0));
    assert_eq!(g.app().editor.doc.len(), 1);
    g.double_click_label("00");
    assert!(g.app().cut_settings_open.is_some(), "double-clicking a layer opens its cut settings");
    g.close_dialogs();
    // The wheel zooms about the pointer.
    let before = g.app().prefs.view.zoom;
    let at = g.canvas_rect().center();
    g.scroll(at, egui::vec2(0.0, 120.0));
    assert!(g.app().prefs.view.zoom > before, "zoom {before} -> {}", g.app().prefs.view.zoom);
    // Resizing the window keeps everything on screen.
    g.set_window_size(1100.0, 700.0);
    assert_eq!(g.screen().size(), egui::vec2(1100.0, 700.0));
    assert!(g.layout_problems().is_empty(), "{:?}", g.layout_problems());
}

#[test]
fn the_widget_registry_does_not_grow_frame_after_frame() {
    let mut g = Gui::standard();
    g.frames(5);
    let n = g.rects("").len();
    g.frames(60);
    assert_eq!(g.rects("").len(), n, "the registry keeps last frame's rectangles only");
}

#[test]
fn the_default_layout_matches_its_golden_picture() {
    let mut g = Gui::new(1280.0, 720.0, 1.0);
    g.frames(3);
    g.check_golden("default-1280x720", 0.005);
}

#[test]
fn no_file_dialog_opens_for_real_and_the_scripted_ones_are_logged() {
    let mut g = Gui::standard();
    g.dialog_log();
    g.open_menu("File", "Open");
    g.open_menu("File", "Save As");
    let log = g.dialog_log();
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].starts_with("pick_file") && log[1].starts_with("save_file"), "{log:?}");
    assert!(g.app().project_path.is_none(), "a cancelled dialog saves nothing");
}

/// Every control in every panel has a name a screen reader can say (a value alone is not a name).
/// The dialogs are covered by `ui_bug_tests::every_control_in_every_window_has_an_accessible_name`.
#[test]
fn every_interactive_control_has_an_accessible_name() {
    let mut g = Gui::standard();
    g.sample_design();
    let mut bad = std::collections::BTreeSet::new();
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    for p in crate::panels::PANELS {
        show_tab(&mut g, p.id);
        for n in g.a11y() {
            let interactive = ["SpinButton", "Button", "CheckBox", "TextInput", "ComboBox", "Slider", "Link"].contains(&n.role.as_str());
            if interactive && n.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                bad.insert(format!("{} in panel {} at {:?}", n.role, p.id, n.rect));
            }
        }
    }
    assert!(bad.is_empty(), "{} controls have no accessible name:\n{}", bad.len(), bad.into_iter().collect::<Vec<_>>().join("\n"));
}

// ----- (a) every menu, every item -----

/// What a menu shows for each leaf: the path of texts to click.
fn leaves(entries: &[Entry], prefix: &[String], out: &mut Vec<(Vec<String>, String)>) {
    for e in entries {
        match e {
            Entry::Cmd(id) => {
                if let Some(c) = commands::find(id) {
                    let mut p = prefix.to_vec();
                    p.push(crate::i18n::cmd_label(c));
                    out.push((p, id.to_string()));
                }
            }
            Entry::Sub(label, inner) => {
                let mut p = prefix.to_vec();
                p.push(crate::i18n::tr(&format!("menu.{label}"), label));
                leaves(inner, &p, out);
            }
            Entry::Dynamic("panels") => {
                for pd in crate::panels::PANELS {
                    let mut p = prefix.to_vec();
                    p.push(crate::i18n::panel_title(pd.id, pd.title));
                    out.push((p, format!("panel.{}", pd.id)));
                }
            }
            Entry::Dynamic("toolbars") => {
                for (bar, label) in crate::prefs::TOOLBARS.iter().copied().chain([("status", "Status Bar")]) {
                    let mut p = prefix.to_vec();
                    p.push(crate::i18n::tr(&format!("toolbar.{bar}"), label));
                    out.push((p, format!("toolbar.{bar}")));
                }
            }
            Entry::Dynamic("languages") => {
                for (code, name) in commands::LANGUAGES {
                    if crate::i18n::available(code) {
                        let mut p = prefix.to_vec();
                        p.push(name.to_string());
                        out.push((p, format!("lang.{code}")));
                    }
                }
            }
            Entry::Dynamic(_) | Entry::Sep => {}
        }
    }
}

fn every_item_of(menu: &str) {
    let (_, entries) = commands::MENUS.iter().find(|(n, _)| *n == menu).unwrap_or_else(|| panic!("no menu {menu}"));
    let mut items = Vec::new();
    leaves(entries, &[], &mut items);
    assert!(!items.is_empty(), "{menu} has no items");
    let mut g = Gui::standard();
    for (path, id) in items {
        g.close_dialogs();
        g.escape();
        g.open_menu(menu, &path.join(" > "));
        g.frames(3);
        eprintln!("{menu} > {} ({id}) ok", path.join(" > "));
        g.close_dialogs();
    }
    g.screenshot(&format!("menu-{}", menu.to_lowercase().replace(' ', "-")));
}

macro_rules! menu_tests {
    ($($name:ident => $menu:expr),* $(,)?) => {
        $(#[test] fn $name() { every_item_of($menu); })*
        #[test]
        fn every_menu_has_a_smoke_test() {
            let tested = [$($menu),*];
            let all: Vec<&str> = commands::MENUS.iter().map(|(n, _)| *n).collect();
            assert_eq!(all, tested, "a menu was added or renamed: add it to menu_tests! in ui_smoke_tests.rs");
        }
    };
}

// ----- (b) every ribbon tab and button, every tool -----

/// The command buttons the ribbon published for the tab on show.
fn ribbon_buttons(g: &Gui) -> Vec<String> {
    g.rects("ribbon:")
        .into_iter()
        .filter_map(|(n, _)| n.strip_prefix("ribbon:").map(str::to_string))
        .filter(|n| n.contains('.') && !n.starts_with("tab:"))
        .collect()
}

#[test]
fn every_ribbon_button_on_every_tab_can_be_clicked() {
    let mut g = Gui::standard();
    g.sample_design();
    let tabs: Vec<String> = g.rects("ribbon:tab:").into_iter().filter_map(|(n, _)| n.strip_prefix("ribbon:tab:").map(str::to_string)).collect();
    assert!(tabs.len() >= 5, "the ribbon shows its tabs: {tabs:?}");
    let mut clicked = 0;
    for tab in &tabs {
        g.click_id(&format!("ribbon:tab:{tab}"));
        assert_eq!(g.app().prefs.ribbon_tab, *tab);
        let buttons = ribbon_buttons(&g);
        for id in buttons {
            g.close_dialogs();
            g.sample_design();
            g.click_id(&format!("ribbon:{id}"));
            g.frames(2);
            clicked += 1;
            g.close_dialogs();
            // The click may have changed the tab (it never should) or the tool: go back.
            if g.app().prefs.ribbon_tab != *tab {
                g.click_id(&format!("ribbon:tab:{tab}"));
            }
        }
        g.screenshot(&format!("ribbon-tab-{tab}"));
    }
    assert!(clicked >= 40, "clicked {clicked} ribbon buttons");
    // Every ribbon command exists in the registry.
    for id in crate::ribbon::command_ids() {
        assert!(commands::find(id).is_some(), "{id}");
    }
}

#[test]
fn every_tool_in_the_tool_column_can_be_picked_and_the_canvas_used() {
    let mut g = Gui::standard();
    g.sample_design();
    for id in crate::shell::tool_ids() {
        g.close_dialogs();
        g.click_id(&format!("tool:{id}"));
        if ["tools.select", "tools.line", "tools.rect", "tools.ellipse", "tools.polygon", "tools.nodes", "tools.text", "tools.measure"].contains(id) {
            let tool = g.app().tool;
            assert_eq!(tool.command(), *id, "clicking the {id} button picks that tool");
        }
        // Use the tool on the canvas: a click and a drag.
        g.click_mm(120.0, 200.0);
        g.drag_mm((60.0, 150.0), (110.0, 190.0));
        g.escape();
        g.close_dialogs();
    }
}

// ----- (c) every panel shown and hidden, every tab cycled -----

/// Bring panel `id` to the front of its dock by clicking its tab (or picking it from the chevron).
fn show_tab(g: &mut Gui, id: &str) {
    let area = g.app().prefs.panel(id).area;
    if g.rect_of(&format!("panel:tab:{id}")).is_some() {
        g.click_id(&format!("panel:tab:{id}"));
    } else {
        g.click_id(&format!("panel:overflow:{area:?}"));
        let title = crate::panels::PANELS.iter().find(|p| p.id == id).map(|p| crate::i18n::panel_title(p.id, p.title)).unwrap();
        g.click_label(&title);
    }
    assert_eq!(g.app().prefs.active_tab.get(&area).map(String::as_str), Some(id), "the {id} tab is in front");
}

#[test]
fn every_panel_can_be_shown_hidden_and_cycled() {
    let mut g = Gui::new(1920.0, 1080.0, 1.0);
    g.sample_design();
    // Show every panel (the Window menu's items, here by command, which is what the menu runs).
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
        assert!(g.app().prefs.panel(p.id).visible, "{} is shown", p.id);
    }
    g.frames(3);
    g.screenshot("panels-all-shown");
    // Cycle every tab twice, with a shape selected and with nothing selected.
    for round in 0..2 {
        for p in crate::panels::PANELS {
            show_tab(&mut g, p.id);
            g.frames(2);
            if round == 1 {
                g.close_dialogs();
            }
        }
        g.exec("edit.invert_selection", serde_json::json!({}));
        if round == 0 {
            g.app().editor.selection.clear();
        }
    }
    // Hide each one again with the dock's close button, then with the command.
    g.screenshot("panels-last-tab");
    for p in crate::panels::PANELS {
        let before = g.app().prefs.panel(p.id).visible;
        assert!(before);
        g.run_command(&format!("panel.{}", p.id));
        assert!(!g.app().prefs.panel(p.id).visible, "{} is hidden", p.id);
    }
    g.screenshot("panels-all-hidden");
    g.run_command("window.reset_layout");
    assert!(g.app().prefs.panel("cuts").visible);
}

#[test]
fn every_toolbar_and_the_status_bar_can_be_toggled() {
    let mut g = Gui::standard();
    for (bar, _) in crate::prefs::TOOLBARS.iter().copied().chain([("status", "")]) {
        g.run_command(&format!("toolbar.{bar}"));
        g.frames(2);
        g.run_command(&format!("toolbar.{bar}"));
    }
    // All off, then all on.
    for (bar, _) in crate::prefs::TOOLBARS {
        g.run_command(&format!("toolbar.{bar}"));
    }
    g.screenshot("toolbars-all-off");
    for (bar, _) in crate::prefs::TOOLBARS {
        g.run_command(&format!("toolbar.{bar}"));
    }
    g.frames(3);
}

// ----- (e) no clipping at any window size -----

/// Window sizes in physical pixels, at each scale. A window cannot be smaller than the app's
/// minimum (960 x 600 points), so small sizes at a high scale are raised to it.
fn layout_at(px_w: f32, px_h: f32, scale: f32, shots: bool) -> Vec<String> {
    let (w, h) = ((px_w / scale).max(960.0), (px_h / scale).max(600.0));
    let mut g = Gui::new(w, h, scale);
    g.sample_design();
    let mut problems: Vec<String> = Vec::new();
    // The default layout, then with every panel shown (the most crowded dock layout).
    problems.extend(g.layout_problems().into_iter().map(|p| format!("default layout: {p}")));
    if shots {
        g.screenshot(&format!("layout-{px_w}x{px_h}@{scale}"));
    }
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    g.frames(3);
    problems.extend(g.layout_problems().into_iter().map(|p| format!("all panels: {p}")));
    for tab in ["home", "arrange", "modify", "text", "modes"] {
        g.click_id(&format!("ribbon:tab:{tab}"));
        problems.extend(g.layout_problems().into_iter().map(|p| format!("ribbon tab {tab}: {p}")));
    }
    if shots {
        g.screenshot(&format!("layout-{px_w}x{px_h}@{scale}-all-panels"));
    }
    problems.sort();
    problems.dedup();
    problems
}

fn layout_cases(sizes: &[(f32, f32)]) {
    let mut all = Vec::new();
    for &(w, h) in sizes {
        for scale in [1.0, 2.0] {
            for p in layout_at(w, h, scale, true) {
                all.push(format!("{w}x{h} @{scale}: {p}"));
            }
        }
    }
    assert!(all.is_empty(), "{} layout problems:\n{}", all.len(), all.join("\n"));
}

#[test]
fn nothing_clips_at_1280x720_and_1920x1080() {
    layout_cases(&[(1280.0, 720.0), (1920.0, 1080.0)]);
}

#[test]
fn nothing_clips_at_2560x1440() {
    layout_cases(&[(2560.0, 1440.0)]);
}

#[test]
fn nothing_clips_at_3840x2160() {
    // The 4K pictures are 8 million pixels each; they take a second to render and write.
    layout_cases(&[(3840.0, 2160.0)]);
}

// ----- (d) every dialog window: draws on screen, closes -----

/// Open windows: (title, rect) from the accessibility tree.
fn open_windows(g: &Gui) -> Vec<(String, egui::Rect)> {
    g.a11y().into_iter().filter(|n| n.role == "Window").map(|n| (n.label.unwrap_or_default(), n.rect)).collect()
}

/// Check the windows now open sit inside the screen; record their titles.
fn check_windows(g: &Gui, what: &str, seen: &mut std::collections::BTreeSet<String>) {
    let screen = g.screen();
    for (title, r) in open_windows(g) {
        assert!(!crate::ui_harness::outside(screen, r), "{what}: window {title:?} {r:?} is not inside the screen {screen:?}");
        seen.insert(title);
    }
    for r in g.dialog_windows() {
        assert!(!crate::ui_harness::outside(screen, r), "{what}: a dialog {r:?} is not inside the screen {screen:?}");
    }
}

fn fresh_design(g: &mut Gui) {
    g.close_dialogs();
    crate::files::new_project(&mut g.app());
    g.sample_design();
}

/// Every registered command, run as the menus run it: the dialogs it opens fit the window and close.
fn sweep_commands(w: f32, h: f32, seen: &mut std::collections::BTreeSet<String>) {
    let mut g = Gui::new(w, h, 1.0);
    let ids: Vec<&str> = commands::COMMANDS.iter().chain(crate::machine_ui::COMMANDS).map(|c| c.id).collect();
    for id in ids {
        fresh_design(&mut g);
        g.run_command(id);
        check_windows(&g, id, seen);
        g.close_dialogs();
    }
}

/// The windows nothing in a menu opens by itself: set the state that shows each one.
pub(crate) type OpenDialog = (&'static str, Box<dyn Fn(&mut crate::shell::DarkBurn)>);

/// Each window that opens from state alone, with the state that shows it.
pub(crate) fn state_dialog_cases() -> Vec<OpenDialog> {
    use crate::devices::{Candidate, DeviceType, Wizard, WizardStep};
    let mut cases: Vec<OpenDialog> = vec![
        ("about", Box::new(|a| a.about_open = true)),
        ("settings", Box::new(|a| a.settings_open = true)),
        ("notes", Box::new(|a| a.notes_open = true)),
        ("cut settings editor", Box::new(|a| a.cut_settings_open = Some(0))),
        ("optimization settings", Box::new(|a| a.job.optimization_open = true)),
        ("gcode settings", Box::new(|a| a.job.gcode_open = true)),
        ("job message", Box::new(|a| a.job.message = Some(crate::job_cmds::Message { title: "No job".into(), text: "Nothing to run.".into() }))),
        ("file message", Box::new(|a| crate::files::show_message(a, "Could not open", "x.dbrn: the file is damaged".into()))),
        ("import warnings", Box::new(|a| a.fio.warnings = Some(vec![("a.dxf".into(), vec!["Spline approximated".into(); 12])]))),
        ("devices", Box::new(|a| a.machine.dlg.devices = true)),
        ("macro", Box::new(|a| a.machine.dlg.macro_edit = Some((0, "G0 X0 Y0".into())))),
        ("controller info", Box::new(|a| a.machine.dlg.controller_info = true)),
        ("machine settings", Box::new(|a| a.machine.dlg.machine_settings = true)),
        (
            "find my laser",
            Box::new(|a| {
                a.machine.dlg.find =
                    Some(vec![Candidate { port: "COM3".into(), label: "USB serial".into(), kind: DeviceType::Grbl, note: "GRBL at 115200".into() }])
            }),
        ),
    ];
    for step in WizardStep::ALL {
        cases.push((
            "device wizard",
            Box::new(move |a| {
                let mut wiz = Wizard::new();
                wiz.step = step;
                a.machine.dlg.wizard = Some(wiz);
            }),
        ));
    }
    cases
}

fn state_dialogs(w: f32, h: f32, seen: &mut std::collections::BTreeSet<String>) {
    let mut g = Gui::new(w, h, 1.0);
    for (what, open) in state_dialog_cases() {
        fresh_design(&mut g);
        open(&mut g.app());
        g.frames(3);
        assert!(g.dialog_open(), "{what}: no window opened");
        check_windows(&g, what, seen);
        g.close_dialogs();
    }
}

fn dialog_sizes() -> [(f32, f32); 3] {
    // 960 x 600 points is the smallest window the app allows.
    [(960.0, 600.0), (1280.0, 720.0), (1920.0, 1080.0)]
}

#[test]
fn every_command_opens_dialogs_that_fit_and_close() {
    let mut seen = std::collections::BTreeSet::new();
    for (w, h) in dialog_sizes() {
        sweep_commands(w, h, &mut seen);
    }
    eprintln!("windows seen: {seen:?}");
    assert!(seen.len() >= 15, "windows seen: {seen:?}");
}

#[test]
fn every_state_driven_dialog_fits_and_closes() {
    let mut seen = std::collections::BTreeSet::new();
    for (w, h) in dialog_sizes() {
        state_dialogs(w, h, &mut seen);
    }
    for want in
        ["About DarkBurn", "Settings", "Project notes", "Cut Settings Editor", "Devices", "Find my laser", "Machine Settings", "Before you start"]
    {
        if want == "Before you start" {
            continue;
        }
        assert!(seen.contains(want), "{want} was not seen: {seen:?}");
    }
}

// ----- (f) a user story, start to finish, through the real UI -----

/// Draw frames until `done` holds (worker threads build jobs and read files meanwhile).
#[track_caller]
fn wait_until(g: &mut Gui, what: &str, done: impl Fn(&mut Gui) -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        g.frames(2);
        if done(g) {
            return;
        }
        assert!(std::time::Instant::now() < end, "timed out waiting for {what}; texts: {:?}", g.visible_texts());
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[test]
fn user_story_draw_set_up_preview_save_undo_and_reopen() {
    use egui::{Key, Modifiers};
    let mut g = Gui::new(1600.0, 1000.0, 1.0);

    // New project (the ribbon's New button).
    g.click_id("ribbon:file.new");
    assert_eq!(g.app().editor.doc.len(), 0);

    // Draw a rectangle with the tool: pick it in the tool column, drag on the canvas.
    g.click_id("tool:tools.rect");
    assert_eq!(g.app().tool.command(), "tools.rect");
    g.drag_mm((40.0, 60.0), (120.0, 110.0));
    assert_eq!(g.app().editor.doc.len(), 1, "one rectangle was drawn");
    let bb = g.app().editor.doc.bbox().unwrap();
    assert!((bb.width() - 80.0).abs() < 1.5 && (bb.height() - 50.0).abs() < 1.5, "drawn 80 x 50 mm: {bb:?}");
    g.screenshot("story-1-drawn");

    // Layer mode and speed in Cuts / Layers: the shape went on layer 0.
    let layer = g.app().editor.doc.iter().next().unwrap().layer;
    assert_eq!(g.app().editor.doc.layer(layer).main().mode, darkburn_core::Mode::Line);
    show_tab(&mut g, "cuts");
    g.click_id(&format!("dialog:control:{}", "Speed"));
    g.key(Key::A, Modifiers::COMMAND);
    g.type_text("250");
    g.enter();
    assert!((g.app().editor.doc.layer(layer).main().speed - 250.0).abs() < 1e-6, "speed is {}", g.app().editor.doc.layer(layer).main().speed);
    // The mode drop-down of the layer's row.
    g.click_label("Line");
    g.click_label("Fill");
    assert_eq!(g.app().editor.doc.layer(layer).main().mode, darkburn_core::Mode::Fill);
    g.screenshot("story-2-layer");

    // Preview: opens, builds the job, shows its summary.
    g.click_id("ribbon:window.preview");
    wait_until(&mut g, "the preview job", |g| g.app().job.preview.data.is_some());
    assert!(g.window_titles().iter().any(|t| t == "Preview"), "{:?}", g.window_titles());
    assert!(g.has_text("Close"));
    g.screenshot("story-3-preview");
    g.close_dialogs();

    // Save G-code to a file in the temp folder (the Laser panel's button, the file dialog scripted).
    let gcode = g.temp("story.gc");
    g.answer_dialog(&gcode);
    show_tab(&mut g, "laser");
    // The button is below the fold of the Laser panel: scroll the panel until it is on screen.
    for _ in 0..10 {
        let r = g.rect_of("dialog:button::Save G-code").unwrap();
        if r.bottom() < g.screen().bottom() - 40.0 {
            break;
        }
        let at = g.rect_of("dialog:button::Connect").unwrap().center();
        g.scroll(at, egui::vec2(0.0, -120.0));
    }
    g.click_id("dialog:button::Save G-code");
    wait_until(&mut g, "the G-code file", |_| gcode.exists());
    let text = std::fs::read_to_string(&gcode).unwrap();
    assert!(text.contains("G1") || text.contains("G01"), "G-code has moves: {}", &text[..text.len().min(200)]);
    assert!(text.contains("S"), "G-code sets the power");
    g.close_dialogs();

    // Undo and redo with the keys, then with the ribbon buttons.
    g.key(Key::Z, Modifiers::COMMAND);
    g.frames(2);
    let after_undo = g.app().editor.doc.layer(layer).main().mode;
    g.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(
        g.app().editor.doc.layer(layer).main().mode,
        darkburn_core::Mode::Fill,
        "redo brings the Fill mode back (undone mode was {after_undo:?})"
    );
    g.click_id("ribbon:edit.undo");
    g.click_id("ribbon:edit.redo");
    assert_eq!(g.app().editor.doc.layer(layer).main().mode, darkburn_core::Mode::Fill);

    // Save the project, start a new one, open it again.
    let project = g.temp("story.dbrn");
    g.answer_dialog(&project);
    g.key(Key::S, Modifiers::COMMAND);
    wait_until(&mut g, "the project file", |_| project.exists());
    assert_eq!(g.app().project_path.as_deref(), Some(project.as_path()));
    g.click_id("ribbon:file.new");
    g.close_dialogs();
    assert_eq!(g.app().editor.doc.len(), 0);
    g.answer_dialog(&project);
    g.key(Key::O, Modifiers::COMMAND);
    wait_until(&mut g, "the project to open", |g| g.app().editor.doc.len() == 1);
    let again = g.app().editor.doc.bbox().unwrap();
    assert!((again.width() - bb.width()).abs() < 1e-6 && (again.height() - bb.height()).abs() < 1e-6, "{again:?} vs {bb:?}");
    let main = g.app().editor.doc.layer(layer).main().clone();
    assert_eq!(main.mode, darkburn_core::Mode::Fill);
    assert!((main.speed - 250.0).abs() < 1e-6);
    g.screenshot("story-4-reopened");
}

menu_tests! {
    menu_file => "File",
    menu_edit => "Edit",
    menu_tools => "Tools",
    menu_arrange => "Arrange",
    menu_laser_tools => "Laser Tools",
    menu_window => "Window",
    menu_language => "Language",
    menu_help => "Help",
}

#[test]
fn closing_dialogs_waits_for_a_job_build_that_is_slow_to_stop() {
    use std::time::{Duration, Instant};
    let mut g = Gui::standard();
    {
        // A job that has been in preparation for a while, on a worker that takes its time to
        // notice Cancel (as on a busy computer).
        let mut a = g.app();
        let task = crate::job_build::BuildTask::slow_to_cancel(Duration::from_millis(1500));
        a.job.pending = Some((task, crate::job_cmds::After::Preview));
        a.job.pending_started = Instant::now().checked_sub(Duration::from_secs(2));
    }
    g.frames(2);
    assert!(g.window_titles().iter().any(|t| t == "Preparing the job"), "{:?}", g.window_titles());
    g.close_dialogs();
    assert!(g.app().job.pending.is_none(), "the build has ended");
    assert!(!g.dialog_open());
}

#[test]
fn a_language_switch_in_one_test_does_not_reach_another() {
    crate::i18n::set_language("nb");
    assert_eq!(crate::i18n::language(), "nb");
    // Another test is another thread with its own app.
    let other = std::thread::spawn(crate::i18n::language).join().unwrap();
    assert_eq!(other, "en");
}

// A settings window docks as a tab when dropped on a panel area, floats again when its tab is
// dragged out, resizes from its corner and opens in a window of its own from the header menu.
#[test]
fn a_settings_window_docks_floats_resizes_and_detaches() {
    use darkburn_ui::ui_kit::window_host::Placement;
    let placement = |g: &Gui| g.app().prefs.windows.get("settings").map(|p| p.placement.clone()).unwrap_or_default();
    let mut g = Gui::standard();
    g.app().settings_open = true;
    g.frames(3);
    let label = "Play a sound on errors";
    assert!(g.has_text(label), "{:?}", g.visible_texts());
    let win = g.rect_of("dialog:window").expect("the Settings window");
    let area = g.rect_of("panel:area:RightTop").expect("the right panel area");

    // Drag it by its header onto the right panel area: it becomes a tab there.
    g.drag(egui::pos2(win.left() + 120.0, win.top() + 12.0), area.center());
    assert_eq!(placement(&g), Placement::Docked("RightTop".into()));
    g.frames(3);
    assert!(g.rect_of("panel:tab:window:settings").is_some(), "{:?}", g.rects("panel:tab:"));
    let area = g.rect_of("panel:area:RightTop").unwrap();
    // The tab is shorter than the window was: its first rows show, the rest scroll.
    let at = g.find_text("Lengths").unwrap_or_else(|| panic!("the settings show in the tab: {:?}", g.visible_texts()));
    assert!(area.contains(at.center()), "{at:?} in {area:?}");
    assert!(!g.window_titles().iter().any(|t| t == "Settings"), "no floating window while docked");
    g.screenshot("settings-docked");

    // Drag the tab onto the canvas: it floats there.
    let tab = g.rect_of("panel:tab:window:settings").unwrap();
    let to = g.canvas_rect().center();
    g.drag(tab.center(), to);
    assert_eq!(placement(&g), Placement::Floating);
    g.frames(3);
    let before = g.rect_of("dialog:window").expect("floating again");
    assert!(g.rect_of("panel:tab:window:settings").is_none());

    // Drag the right edge: the window grows and remembers its size. (Its bottom corner sits on
    // the app window's own resize border here, which is on top, so the edge is the grip to use.)
    assert!(g.rect_of("dialog:resize:corner").is_some(), "a resize corner");
    let edge = g.rect_of("dialog:resize:right").expect("a resize edge");
    g.drag(edge.center(), edge.center() + egui::vec2(80.0, 0.0));
    g.frames(2);
    let after = g.rect_of("dialog:window").unwrap();
    assert!(after.width() > before.width() + 40.0, "{before:?} -> {after:?}");
    assert!(g.app().prefs.windows["settings"].size.is_some());
    assert!(g.has_text("Lengths"));

    // The header menu opens it in a window of its own (the test harness shows that inside).
    g.click_id("dialog:place");
    g.click_label("Open in its own window");
    assert_eq!(placement(&g), Placement::Detached);
    g.frames(3);
    assert!(g.has_text("Lengths"), "{:?}", g.visible_texts());
    g.screenshot("settings-detached-fallback");

    // Closed and opened again, it comes back where it was left.
    g.click_label("Close");
    assert!(!g.app().settings_open);
    g.app().settings_open = true;
    g.frames(3);
    assert_eq!(placement(&g), Placement::Detached);
}

/// Edit Hotkeys as a tab, waiting for a key, and another tab clicked in front of it: the wait
/// ends. It went on unseen and took the next key pressed anywhere.
#[test]
fn a_hidden_hotkey_tab_stops_waiting_for_a_key() {
    use darkburn_ui::ui_kit::window_host::WindowPlace;
    let mut g = Gui::standard();
    let mut place = WindowPlace::default();
    place.dock("RightTop");
    g.app().prefs.windows.insert("db_hotkeys".into(), place);
    g.app().extras.hotkeys.open = true;
    g.frames(3);
    g.click_id("panel:tab:window:db_hotkeys");
    g.app().extras.hotkeys.capturing = Some("tools.rect".into());
    g.frames(2);
    assert!(g.app().extras.hotkeys.capturing.is_some(), "in front, it waits");
    let area = g.rect_of("panel:area:RightTop").expect("the right panel area");
    let other = g
        .rects("panel:tab:")
        .into_iter()
        .find(|(name, r)| name != "panel:tab:window:db_hotkeys" && area.contains(r.center()))
        .unwrap_or_else(|| panic!("another tab in the area: {:?}", g.rects("panel:tab:")));
    g.click_pos(other.1.center());
    g.frames(2);
    assert_eq!(g.app().extras.hotkeys.capturing, None, "behind {}, it waits no more", other.0);
    assert!(g.app().extras.hotkeys.open, "the window is still open");
}

/// A docked window open behind another tab comes to the front when it is opened again: Settings
/// from its command, the Cut Settings Editor from the Cuts panel's button. Their open flag was set
/// already, so nothing could be seen to happen.
#[test]
fn opening_a_docked_window_that_is_open_brings_its_tab_to_the_front() {
    use crate::panels::Area;
    use darkburn_ui::ui_kit::window_host::{WindowPlace, tab_id};
    let mut g = Gui::standard();
    g.sample_design();
    for key in ["settings", crate::cut_settings::WINDOW_KEY] {
        let mut place = WindowPlace::default();
        place.dock("RightTop");
        g.app().prefs.windows.insert(key.into(), place);
    }
    let front = |g: &Gui| g.app().prefs.active_tab.get(&Area::RightTop).cloned().unwrap_or_default();
    g.run_command("edit.settings");
    assert_eq!(front(&g), tab_id("settings"));
    g.app().prefs.active_tab.insert(Area::RightTop, "cuts".into());
    g.frames(2);
    g.run_command("edit.settings");
    assert_eq!(front(&g), tab_id("settings"), "Settings, open behind the Cuts / Layers tab");

    g.app().prefs.active_tab.insert(Area::RightTop, "cuts".into());
    g.frames(2);
    g.click_label("Edit cut settings");
    g.frames(2);
    assert_eq!(front(&g), tab_id(crate::cut_settings::WINDOW_KEY));
    g.app().prefs.active_tab.insert(Area::RightTop, "cuts".into());
    g.frames(2);
    assert!(g.app().cut_settings_open.is_some(), "the editor stays open behind the tab");
    g.click_label("Edit cut settings");
    g.frames(2);
    assert_eq!(front(&g), tab_id(crate::cut_settings::WINDOW_KEY), "the editor, open behind the Cuts / Layers tab");
}

/// Closing a docked window brings back the tab it covered, not the area's first one: with the
/// panels moved, that can be the Laser panel with its Stop button.
// Every window a command opens, docked in each panel area in turn (the left and bottom areas hold
// no panel by default) and then in a window of its own (the harness shows that inside): it shows
// with no widget id clash (the harness fails on one), its footer and buttons inside the area, and
// the close button of its tab closes it.
#[test]
fn every_dockable_window_docks_in_each_area_detaches_and_closes() {
    use crate::panels::Area;
    use darkburn_ui::ui_kit::window_host::{WindowPlace, tab_id};
    let mut g = Gui::standard();
    // Adjust Image needs a picture selected.
    let file = g.temp("ramp.png");
    image::GrayImage::from_fn(90, 60, |x, y| image::Luma([((x * 2 + y * 3) % 256) as u8])).save(&file).expect("write the picture");
    let r = g.exec("project.import", serde_json::json!({ "paths": [file.to_string_lossy()], "x": 100, "y": 100 }));
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    let mut footers = 0;
    for (cmd, key) in crate::shell::WINDOW_COMMANDS {
        let tab = format!("panel:tab:{}", tab_id(key));
        for area in Area::ALL {
            let what = format!("{key} docked {area:?}");
            let mut place = WindowPlace::default();
            place.dock(&format!("{area:?}"));
            g.app().prefs.windows.insert(key.to_string(), place);
            g.run_command(cmd);
            let dock = g.rect_of(&format!("panel:area:{area:?}")).unwrap_or_else(|| panic!("{what}: the area shows: {:?}", g.rects("panel:area:")));
            assert!(g.rect_of(&tab).is_some(), "{what}: its tab: {:?}", g.rects("panel:tab:"));
            let window = g.rect_of("dialog:window").unwrap_or_else(|| panic!("{what}: it shows"));
            let inside = dock.expand(1.5);
            assert!(inside.contains_rect(window), "{what}: {window:?} in {dock:?}");
            for (name, r) in g
                .rects("dialog:footer")
                .into_iter()
                .chain(["dialog:ok", "dialog:cancel"].into_iter().filter_map(|n| g.rect_of(n).map(|r| (n.to_string(), r))))
            {
                assert!(inside.contains_rect(r), "{what}: {name} {r:?} outside {dock:?}");
                footers += 1;
            }
            g.click_a11y(Some("Button"), "Close this window");
            g.frames(3);
            assert!(g.rect_of(&tab).is_none(), "{what}: the tab's close button closed it");
            assert!(g.rect_of("dialog:window").is_none(), "{what}: closed");
        }
        let mut place = WindowPlace::default();
        place.detach();
        g.app().prefs.windows.insert(key.to_string(), place);
        g.run_command(cmd);
        assert!(g.rect_of("dialog:window").is_some(), "{key} in a window of its own");
        assert!(g.rect_of(&tab).is_none(), "{key}: no tab while in a window of its own");
        g.close_dialogs();
        assert!(g.rect_of("dialog:window").is_none(), "{key}: closed");
    }
    assert!(footers > 0, "no window showed a footer");
}

// Two windows docked in one area are two tabs, and closing one leaves the other. A window stored
// as docked in an area this version does not have opens floating instead.
#[test]
fn two_windows_share_an_area_and_an_unknown_area_floats() {
    use darkburn_ui::ui_kit::window_host::{Placement, WindowPlace, tab_id};
    let mut g = Gui::standard();
    for key in ["settings", "db_hotkeys"] {
        let mut place = WindowPlace::default();
        place.dock("Bottom");
        g.app().prefs.windows.insert(key.into(), place);
    }
    g.run_command("edit.settings");
    g.run_command("file.hotkeys");
    let tabs = |g: &Gui| g.rects("panel:tab:window:").into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    let both = tabs(&g);
    assert!(both.contains(&format!("panel:tab:{}", tab_id("settings"))) && both.contains(&format!("panel:tab:{}", tab_id("db_hotkeys"))), "{both:?}");
    let area = g.rect_of("panel:area:Bottom").expect("the bottom area, for its two windows");
    for (name, r) in g.rects("panel:tab:window:") {
        assert!(area.expand(1.5).contains_rect(r), "{name} {r:?} in {area:?}");
    }
    // Edit Hotkeys came last, so it is in front: its tab's close button closes it alone.
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(3);
    assert!(!g.app().extras.hotkeys.open, "Edit Hotkeys closed");
    assert!(g.app().settings_open, "Settings stays open");
    assert_eq!(tabs(&g), vec![format!("panel:tab:{}", tab_id("settings"))]);
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(3);
    assert!(!g.app().settings_open);

    let place: WindowPlace = serde_json::from_value(serde_json::json!({ "placement": { "docked": "Middle" } })).expect("a stored place");
    assert_eq!(place.placement, Placement::Docked("Middle".into()));
    g.app().prefs.windows.insert("settings".into(), place);
    g.run_command("edit.settings");
    assert!(tabs(&g).is_empty(), "{:?}", tabs(&g));
    assert!(g.window_titles().iter().any(|t| t == "Settings"), "floating: {:?}", g.window_titles());
}

// In Norwegian the window header menu, the panel areas in it and the close button of a docked
// window's tab are in Norwegian too.
#[test]
fn the_window_menus_follow_the_interface_language() {
    use darkburn_ui::ui_kit::window_host::Placement;
    let mut g = Gui::standard();
    g.app().prefs.language = "nb".into();
    g.app().settings_open = true;
    g.frames(3);
    g.click_id("dialog:place");
    g.frames(1);
    // (The menu heading over the areas is in capitals.)
    for text in ["Åpne i eget vindu", "FEST SOM FANE", "Venstre", "Høyre, øverst", "Høyre, nederst", "Nederst"] {
        assert!(g.has_text(text), "{text}: {:?}", g.visible_texts());
    }
    for text in ["Open in its own window", "DOCK AS TAB", "Right, top"] {
        assert!(!g.has_text(text), "{text} in English: {:?}", g.visible_texts());
    }
    g.click_label("Høyre, øverst");
    assert_eq!(g.app().prefs.windows.get("settings").map(|p| p.placement.clone()), Some(Placement::Docked("RightTop".into())));
    g.frames(3);
    g.click_a11y(Some("Button"), "Lukk dette vinduet");
    g.frames(2);
    assert!(!g.app().settings_open, "the window closed");
}

#[test]
fn closing_a_docked_window_brings_back_the_tab_it_covered() {
    use crate::panels::Area;
    use darkburn_ui::ui_kit::window_host::{WindowPlace, tab_id};
    let mut g = Gui::standard();
    let mut place = WindowPlace::default();
    place.dock("RightTop");
    g.app().prefs.windows.insert("settings".into(), place);
    let front = |g: &Gui| g.app().prefs.active_tab.get(&Area::RightTop).cloned().unwrap_or_default();
    g.app().prefs.active_tab.insert(Area::RightTop, "console".into());
    g.frames(2);
    g.run_command("edit.settings");
    assert_eq!(front(&g), tab_id("settings"));
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(2);
    assert!(!g.app().settings_open, "the window closed");
    assert_eq!(front(&g), "console", "the Console tab it covered is in front again, and stored so");

    // The Laser panel moved up here, behind the first tab (the area holds only these three tabs,
    // so none goes behind its chevron); the window's tab picked by hand.
    for (id, visible) in [("laser", true), ("console", false), ("camera", false), ("variable_text", false), ("shape_properties", false)] {
        let mut app = g.app();
        let panel = app.prefs.panels.get_mut(id).expect(id);
        (panel.area, panel.visible) = (Area::RightTop, visible);
    }
    g.app().prefs.active_tab.insert(Area::RightTop, "laser".into());
    g.frames(2);
    g.run_command("edit.settings");
    assert_eq!(front(&g), tab_id("settings"));
    g.click_id("panel:tab:cuts");
    g.click_id("panel:tab:laser");
    g.click_id(&format!("panel:tab:{}", tab_id("settings")));
    assert_eq!(front(&g), tab_id("settings"));
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(2);
    assert!(!g.app().settings_open);
    assert_eq!(front(&g), "laser", "the Laser panel it covered is in front again");
}

#[test]
fn a_docked_window_wider_than_its_tab_keeps_its_buttons_in_the_tab() {
    use darkburn_ui::ui_kit::window_host::WindowPlace;
    let mut g = Gui::standard();
    let mut place = WindowPlace::default();
    place.dock("RightTop");
    g.app().prefs.windows.insert("db_hotkeys".into(), place);
    // Edit Hotkeys asks for more width than the right panel area has at first.
    g.app().extras.hotkeys.open = true;
    g.frames(3);
    g.click_id("panel:tab:window:db_hotkeys");
    let area = g.rect_of("panel:area:RightTop").expect("the right panel area");
    assert!(area.width() < 560.0, "the area is narrower than the page: {area:?}");
    let close = g.rect_of("dialog:cancel").unwrap_or_else(|| panic!("the Close button: {:?}", g.rects("dialog:")));
    assert!(area.contains_rect(close), "Close {close:?} lies in the tab {area:?}");
    g.click_id("dialog:cancel");
    assert!(!g.app().extras.hotkeys.open, "and it closes the window");
}
