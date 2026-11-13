//! Dockable OK/Cancel windows closed from their tab, their header or their own window while they
//! hold edits not applied ask first: Apply (as OK), Discard (as Cancel) or Keep open. Without
//! edits they close at once, and Esc keeps its meaning (Cancel). Before, the tab's X threw the
//! edits away without a word.

use crate::ui_harness::Gui;
use darkburn_ui::ui_kit::window_host::{self, WindowPlace};

const QUESTION: &str = "This window has changes you have not applied. Apply them before it closes?";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Answer {
    Apply,
    Discard,
    KeepOpen,
}

const ANSWERS: [Answer; 3] = [Answer::Apply, Answer::Discard, Answer::KeepOpen];

impl Answer {
    fn label(self) -> &'static str {
        match self {
            Answer::Apply => "Apply",
            Answer::Discard => "Discard",
            Answer::KeepOpen => "Keep open",
        }
    }
}

/// Window `key` opens docked as a tab in the right top panel area.
fn dock(g: &mut Gui, key: &str) {
    let mut place = WindowPlace::default();
    place.dock("RightTop");
    g.app().prefs.windows.insert(key.into(), place);
}

/// Window `key` opens in a window of its own (in the harness, inside the main window).
fn detach(g: &mut Gui, key: &str) {
    let mut place = WindowPlace::default();
    place.detach();
    g.app().prefs.windows.insert(key.into(), place);
}

fn asking(g: &Gui, key: &str) -> bool {
    window_host::asking(&g.ctx(), key)
}

/// Close docked window `key` from its tab: with edits it asks, and `answer` answers.
fn close_tab_and_answer(g: &mut Gui, key: &str, answer: Answer, what: &str) {
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(2);
    assert!(asking(g, key), "{what}: closing it with edits asks");
    assert!(g.has_text(QUESTION), "{what}: the question shows: {:?}", g.visible_texts());
    g.click_a11y(Some("Button"), answer.label());
    g.frames(3);
    assert!(!asking(g, key), "{what}: answered");
    assert!(!g.has_text(QUESTION), "{what}: the question went");
}

/// Close docked window `key` from its tab with nothing edited: it closes, without a question.
fn close_tab_unedited(g: &mut Gui, key: &str, what: &str) {
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(3);
    assert!(!asking(g, key), "{what}: nothing to ask");
    assert!(!g.has_text(QUESTION), "{what}");
}

// ----- the Cut Settings Editor -----

const CUT_SETTINGS: &str = "cut_settings_window";

/// The Cut Settings Editor, docked, on layer 1 of a small design.
fn cut_settings_docked() -> Gui {
    let mut g = Gui::standard();
    g.sample_design();
    dock(&mut g, CUT_SETTINGS);
    g.app().cut_settings_open = Some(1);
    g.frames(6);
    assert!(g.rect_of(&format!("panel:tab:{}", window_host::tab_id(CUT_SETTINGS))).is_some(), "docked: {:?}", g.rects("panel:tab:"));
    g
}

fn speed(g: &Gui) -> f64 {
    g.app().editor.doc.layer(1).main().speed
}

#[test]
fn the_docked_cut_settings_editor_asks_before_its_tab_closes_it() {
    for answer in ANSWERS {
        let what = format!("{answer:?}");
        let mut g = cut_settings_docked();
        let (before, steps) = (speed(&g), g.app().editor.history.len());
        g.app().editor.doc.layer_mut(1).main_mut().speed = before + 77.0;
        g.frames(2);
        close_tab_and_answer(&mut g, CUT_SETTINGS, answer, &what);
        let open = g.app().cut_settings_open;
        let now = speed(&g);
        let steps_now = g.app().editor.history.len();
        match answer {
            Answer::Apply => {
                assert_eq!(open, None, "{what}: closed");
                assert_eq!(now, before + 77.0, "{what}: the edit is kept");
                assert_eq!(steps_now, steps + 1, "{what}: as one undo step");
            }
            Answer::Discard => {
                assert_eq!(open, None, "{what}: closed");
                assert_eq!(now, before, "{what}: the edit is undone");
                assert_eq!(steps_now, steps, "{what}: no undo step");
            }
            Answer::KeepOpen => {
                assert_eq!(open, Some(1), "{what}: still open");
                assert_eq!(now, before + 77.0, "{what}: with the edit");
                assert!(g.rect_of(&format!("panel:tab:{}", window_host::tab_id(CUT_SETTINGS))).is_some(), "{what}: its tab stays");
                // And it can be closed the same way again.
                close_tab_and_answer(&mut g, CUT_SETTINGS, Answer::Discard, "discarded after keeping it open");
                assert_eq!(g.app().cut_settings_open, None);
                assert_eq!(speed(&g), before);
            }
        }
    }
    let mut g = cut_settings_docked();
    close_tab_unedited(&mut g, CUT_SETTINGS, "no edits");
    assert_eq!(g.app().cut_settings_open, None, "closed at once");
}

/// The picture: the docked Cut Settings Editor asking, its contents waiting under the question.
#[test]
fn the_close_question_over_the_docked_cut_settings_editor() {
    let mut g = cut_settings_docked();
    g.app().editor.doc.layer_mut(1).main_mut().speed += 50.0;
    g.frames(2);
    g.click_a11y(Some("Button"), "Close this window");
    g.frames(4);
    assert!(g.has_text(QUESTION), "{:?}", g.visible_texts());
    for label in ["Apply", "Discard", "Keep open"] {
        assert!(!g.find_a11y(Some("Button"), label).is_empty(), "{label}: {:?}", g.visible_texts());
    }
    let shot = g.screenshot("close-ask-cut-settings-docked");
    assert!(shot.exists());
    assert!(g.layout_problems().iter().all(|p| !p.contains("dialog")), "{:?}", g.layout_problems());
}

/// Esc keeps its meaning: Cancel, at once, without a question. The header's X of a floating
/// window and the close of a window of its own ask like the tab's X.
#[test]
fn esc_still_cancels_and_every_close_button_asks() {
    for place in ["floating", "detached"] {
        let mut g = Gui::standard();
        g.sample_design();
        if place == "detached" {
            detach(&mut g, CUT_SETTINGS);
        }
        g.app().cut_settings_open = Some(1);
        g.frames(6);
        let before = speed(&g);
        g.app().editor.doc.layer_mut(1).main_mut().speed = before + 5.0;
        g.frames(2);
        g.click_a11y(Some("Button"), "Close");
        g.frames(2);
        assert!(asking(&g, CUT_SETTINGS), "{place}: the header's X asks");
        assert!(g.has_text(QUESTION), "{place}: {:?}", g.visible_texts());
        // Esc answers the question (Keep open), not the window under it.
        g.escape();
        g.frames(2);
        assert!(!asking(&g, CUT_SETTINGS), "{place}: Esc keeps it open");
        assert_eq!(g.app().cut_settings_open, Some(1), "{place}: still open");
        assert_eq!(speed(&g), before + 5.0, "{place}: with the edit");
        // Without a question Esc is Cancel, as before.
        if place == "floating" {
            g.escape();
            g.frames(3);
            assert!(!g.has_text(QUESTION), "{place}: Esc does not ask");
            assert_eq!(g.app().cut_settings_open, None, "{place}: Esc cancels");
            assert_eq!(speed(&g), before, "{place}: and puts the layer back");
        }
    }
}

// ----- Optimization, G-code, Device Settings, Adjust Image -----

/// Open `cmd`'s window docked, edit it with `edit`, close its tab and answer; `check` gets the
/// answer and whether the edit is in the saved settings now.
fn docked_window_asks(
    cmd: &str,
    key: &str,
    setup: impl Fn(&mut Gui),
    edit: impl Fn(&mut Gui),
    applied: impl Fn(&Gui) -> bool,
    open: impl Fn(&Gui) -> bool,
) {
    for answer in ANSWERS {
        let what = format!("{key} {answer:?}");
        let mut g = Gui::standard();
        setup(&mut g);
        dock(&mut g, key);
        g.run_command(cmd);
        assert!(open(&g), "{what}: opened");
        edit(&mut g);
        g.frames(2);
        assert!(!applied(&g), "{what}: the edit is a draft until OK");
        close_tab_and_answer(&mut g, key, answer, &what);
        match answer {
            Answer::Apply => {
                assert!(!open(&g), "{what}: closed");
                assert!(applied(&g), "{what}: the edit is applied");
            }
            Answer::Discard => {
                assert!(!open(&g), "{what}: closed");
                assert!(!applied(&g), "{what}: the edit is thrown away");
            }
            Answer::KeepOpen => {
                assert!(open(&g), "{what}: still open");
                assert!(!applied(&g), "{what}: the edit is still a draft");
            }
        }
    }
    let mut g = Gui::standard();
    setup(&mut g);
    dock(&mut g, key);
    g.run_command(cmd);
    close_tab_unedited(&mut g, key, &format!("{key} without edits"));
    assert!(!open(&g), "{key}: closed at once without edits");
}

#[test]
fn the_docked_optimization_window_asks_before_its_tab_closes_it() {
    let planner = |g: &Gui| crate::job_cmds::current_settings(&g.app()).planner;
    let first = Gui::standard();
    let was = planner(&first).reduce_travel;
    drop(first);
    docked_window_asks(
        "job.optimization_settings",
        "db_job_optimization",
        |_| {},
        |g| {
            let mut app = g.app();
            let draft = app.job.optimization_draft.as_mut().expect("the window's draft");
            draft.reduce_travel = !draft.reduce_travel;
        },
        |g| planner(g).reduce_travel != was,
        |g| g.app().job.optimization_open,
    );
}

#[test]
fn the_docked_gcode_window_asks_before_its_tab_closes_it() {
    let first = Gui::standard();
    let was = first.app().job.gcode.auto_home;
    drop(first);
    docked_window_asks(
        "job.gcode_settings",
        "db_job_gcode",
        |_| {},
        |g| {
            let mut app = g.app();
            let draft = app.job.gcode_draft.as_mut().expect("the window's draft");
            draft.0.auto_home = !draft.0.auto_home;
        },
        |g| g.app().job.gcode.auto_home != was,
        |g| g.app().job.gcode_open,
    );
}

#[test]
fn the_docked_device_settings_window_asks_before_its_tab_closes_it() {
    const NAME: &str = "Renamed in the window";
    docked_window_asks(
        "edit.device_settings",
        "db_device_settings",
        |_| {},
        |g| g.app().machine.dlg.settings.as_mut().expect("the window's draft").0.name = NAME.into(),
        |g| g.app().machine.active().name == NAME,
        |g| g.app().machine.dlg.settings.is_some(),
    );
}

/// An image on layer 3, selected.
fn with_image(g: &mut Gui) {
    use crate::images::testutil::{add_image, image_layer, png};
    let mut app = g.app();
    image_layer(&mut app.editor.doc, 3, 0.25);
    let id = add_image(&mut app.editor.doc, png(40, 20, |x, _| if x < 20 { 0 } else { 255 }), (40, 20), (20.0, 10.0), (50.0, 50.0), 3);
    app.editor.selection.set([id]);
}

fn brightness(g: &Gui) -> f64 {
    let app = g.app();
    app.editor
        .doc
        .iter()
        .find_map(|s| match &s.kind {
            darkburn_core::ShapeKind::Image(i) => Some(i.adjust.brightness),
            _ => None,
        })
        .expect("the image")
}

#[test]
fn the_docked_adjust_image_window_asks_before_its_tab_closes_it() {
    for answer in ANSWERS {
        let what = format!("Adjust Image {answer:?}");
        let mut g = Gui::standard();
        with_image(&mut g);
        dock(&mut g, "db_adjust_image");
        g.run_command("tools.adjust_image");
        assert!(g.app().images.adjust.is_some(), "{what}: opened");
        let steps = g.app().editor.history.len();
        g.app().images.adjust.as_mut().expect("open").args.brightness = Some(25.0);
        g.frames(2);
        assert_eq!(brightness(&g), 25.0, "{what}: the edit shows live");
        close_tab_and_answer(&mut g, "db_adjust_image", answer, &what);
        let open = g.app().images.adjust.is_some();
        let now = brightness(&g);
        let steps_now = g.app().editor.history.len();
        match answer {
            Answer::Apply => {
                assert!(!open, "{what}: closed");
                assert_eq!(now, 25.0, "{what}: kept");
                assert_eq!(steps_now, steps + 1, "{what}: as one undo step");
            }
            Answer::Discard => {
                assert!(!open, "{what}: closed");
                assert_eq!(now, 0.0, "{what}: put back");
                assert_eq!(steps_now, steps, "{what}: no undo step");
            }
            Answer::KeepOpen => {
                assert!(open, "{what}: still open");
                assert_eq!(now, 25.0, "{what}: with the edit");
            }
        }
    }
    let mut g = Gui::standard();
    with_image(&mut g);
    dock(&mut g, "db_adjust_image");
    g.run_command("tools.adjust_image");
    close_tab_unedited(&mut g, "db_adjust_image", "Adjust Image without edits");
    assert!(g.app().images.adjust.is_none(), "closed at once without edits");
}

// ----- another project -----

/// Opening another project, or making a new one, while Adjust Image or Optimization is open
/// closes the window and leaves the new project as it came: Cancel or the tab's X used to put
/// the old project's image settings back into the new one, and OK to write its optimization.
#[test]
fn windows_holding_a_projects_draft_close_when_another_project_comes() {
    let mut g = Gui::standard();
    with_image(&mut g);
    g.run_command("tools.adjust_image");
    g.app().images.adjust.as_mut().expect("open").args.brightness = Some(30.0);
    g.frames(2);
    crate::files::new_project(&mut g.app());
    with_image(&mut g);
    let steps = g.app().editor.history.len();
    let modified = g.app().editor.is_modified();
    g.frames(3);
    assert!(g.app().images.adjust.is_none(), "Adjust Image closed");
    assert_eq!(brightness(&g), 0.0, "the new project's image is as it came");
    let steps_now = g.app().editor.history.len();
    let modified_now = g.app().editor.is_modified();
    assert_eq!((steps_now, modified_now), (steps, modified), "and gets no undo step");

    let planner = |g: &Gui| crate::job_cmds::current_settings(&g.app()).planner;
    g.run_command("job.optimization_settings");
    let was = planner(&g);
    {
        let mut app = g.app();
        let draft = app.job.optimization_draft.as_mut().expect("the window's draft");
        draft.reduce_travel = !draft.reduce_travel;
    }
    g.frames(2);
    crate::files::new_project(&mut g.app());
    g.frames(3);
    assert!(!g.app().job.optimization_open, "Optimization closed");
    assert!(g.app().job.optimization_draft.is_none(), "its draft went with it");
    assert_eq!(planner(&g), was, "the new project keeps its own optimization");
}
