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

/// The cut table's column headers in `code`, left to right as painted, with the overlaps
/// between neighbours. A header longer than its narrow column runs into the next one.
fn cut_header_overlaps(code: &str) -> Vec<String> {
    use crate::i18n::tr;
    let mut g = Gui::standard();
    g.run_command(&format!("lang.{code}"));
    g.settle();
    let labels = [
        tr("cuts.col.layer", "Layer"),
        tr("cuts.col.mode", "Mode"),
        tr("cuts.col.speed_power", "Spd/Pwr"),
        tr("cuts.col.out", "Out"),
        tr("cuts.col.show", "Vis"),
        tr("cuts.col.air", "Air"),
    ];
    let painted = g.painted();
    let anchor = painted.iter().find(|p| p.text == labels[2]).map(|p| p.full);
    crate::i18n::set_language("en");
    let Some(anchor) = anchor else {
        return vec![format!("{code}: no cut table header {:?} in {:?}", labels[2], g.visible_texts())];
    };
    let mut rects = Vec::new();
    for l in &labels {
        let near = painted
            .iter()
            .filter(|p| &p.text == l && (p.full.center().y - anchor.center().y).abs() < 2.0 && (p.full.left() - anchor.left()).abs() < 400.0)
            .min_by(|a, b| (a.full.left() - anchor.left()).abs().total_cmp(&(b.full.left() - anchor.left()).abs()));
        match near {
            Some(p) => rects.push((l.clone(), p.full)),
            None => return vec![format!("{code}: cut table header {l:?} is not drawn")],
        }
    }
    let mut out = Vec::new();
    for w in rects.windows(2) {
        let ((a, ra), (b, rb)) = (&w[0], &w[1]);
        if ra.right() > rb.left() - 1.0 {
            out.push(format!("{code}: header {a:?} {ra:?} runs into {b:?} {rb:?}"));
        }
    }
    out
}

/// "Ausg." in German is longer than "Out": every language's headers still end before the
/// next column's header begins [I18N-04].
#[test]
fn cut_table_headers_fit_their_columns_in_every_language() {
    let bad: Vec<String> = ["en", "nb", "de", "fr", "it"].iter().flat_map(|c| cut_header_overlaps(c)).collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
