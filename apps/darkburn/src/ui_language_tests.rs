//! The interface in German, French and Italian: with every panel open, in the settings and in a
//! long help topic, the translated text fits where the English text fits [I18N-04].
use crate::ui_harness::Gui;

fn show_every_panel(g: &mut Gui) {
    for p in crate::panels::PANELS {
        if !g.app().prefs.panel(p.id).visible {
            g.run_command(&format!("panel.{}", p.id));
        }
    }
    g.frames(3);
}

/// Switches to `code`, checks that `edit` (the Edit menu) and `general` (the first settings page)
/// show translated, and returns every layout problem seen. Pictures: `lang-<code>-*.png`.
fn layout_in(code: &str, edit: &str, general: &str, help_title: &str) -> Vec<String> {
    let mut g = Gui::standard();
    g.run_command(&format!("lang.{code}"));
    g.settle();
    assert_eq!(crate::i18n::language(), code);
    assert!(g.has_text(edit), "{code}: no {edit:?} in {:?}", g.visible_texts());
    let mut problems = Vec::new();

    show_every_panel(&mut g);
    g.settle();
    problems.extend(g.layout_problems().into_iter().map(|p| format!("{code}, every panel: {p}")));
    g.screenshot(&format!("lang-{code}-panels"));

    g.run_command("edit.settings");
    g.settle();
    assert!(g.has_text(general), "{code}: no {general:?} in the settings: {:?}", g.visible_texts());
    problems.extend(g.layout_problems().into_iter().map(|p| format!("{code}, settings: {p}")));
    g.screenshot(&format!("lang-{code}-settings"));
    g.close_dialogs();

    g.run_command("help.quick");
    g.app().extras.help_topic = "camera_selection".into();
    g.settle();
    assert!(g.has_text(help_title), "{code}: no {help_title:?} in the help: {:?}", g.visible_texts());
    problems.extend(g.layout_problems().into_iter().map(|p| format!("{code}, help: {p}")));
    g.screenshot(&format!("lang-{code}-help"));
    g.close_dialogs();

    crate::i18n::set_language("en");
    problems
}

#[test]
fn german_fits_the_window() {
    let problems = layout_in("de", "Bearbeiten", "Allgemein", "Eine Kamera wählen");
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn french_fits_the_window() {
    let problems = layout_in("fr", "Édition", "Général", "Choisir une caméra");
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn italian_fits_the_window() {
    let problems = layout_in("it", "Modifica", "Generale", "Scegliere una telecamera");
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
}
