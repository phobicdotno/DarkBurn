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

/// What one view shows in one language: its layout problems and the texts cut short with "...".
struct View {
    name: &'static str,
    problems: Vec<String>,
    elided: Vec<String>,
}

fn look(g: &mut Gui, code: &str, name: &'static str) -> View {
    g.screenshot(&format!("lang-{code}-{name}"));
    let elided = g.painted().into_iter().filter(|p| p.elided).map(|p| p.text).collect();
    View { name, problems: g.layout_problems(), elided }
}

/// Every panel, the settings and the longest help topic in `code`. `edit`, `general` and
/// `help_title` are the Edit menu, the first settings page and the help topic's title in it.
fn views_in(code: &str, edit: &str, general: &str, help_title: &str) -> Vec<View> {
    let mut g = Gui::standard();
    g.run_command(&format!("lang.{code}"));
    g.settle();
    assert_eq!(crate::i18n::language(), code);
    assert!(g.has_text(edit), "{code}: no {edit:?} in {:?}", g.visible_texts());
    let mut out = Vec::new();

    show_every_panel(&mut g);
    g.settle();
    out.push(look(&mut g, code, "panels"));

    g.run_command("edit.settings");
    g.settle();
    assert!(g.has_text(general), "{code}: no {general:?} in the settings: {:?}", g.visible_texts());
    out.push(look(&mut g, code, "settings"));
    g.close_dialogs();

    g.run_command("help.quick");
    g.app().extras.help_topic = "camera_selection".into();
    g.settle();
    assert!(g.has_text(help_title), "{code}: no {help_title:?} in the help: {:?}", g.visible_texts());
    out.push(look(&mut g, code, "help"));
    g.close_dialogs();

    crate::i18n::set_language("en");
    out
}

/// No layout problems, and no more texts cut short in any view than in English.
fn fits_like_english(code: &str, edit: &str, general: &str, help_title: &str) {
    let english = views_in("en", "Edit", "General", "Choosing a camera");
    let views = views_in(code, edit, general, help_title);
    let mut bad = Vec::new();
    for (en, v) in english.iter().zip(&views) {
        bad.extend(v.problems.iter().map(|p| format!("{code}, {}: {p}", v.name)));
        if v.elided.len() > en.elided.len() {
            bad.push(format!("{code}, {}: cut short {:?}, in English only {:?}", v.name, v.elided, en.elided));
        }
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn german_fits_the_window() {
    fits_like_english("de", "Bearbeiten", "Allgemein", "Eine Kamera wählen");
}

#[test]
fn french_fits_the_window() {
    fits_like_english("fr", "Édition", "Général", "Choisir une caméra");
}

#[test]
fn italian_fits_the_window() {
    fits_like_english("it", "Modifica", "Generale", "Scegliere una telecamera");
}
