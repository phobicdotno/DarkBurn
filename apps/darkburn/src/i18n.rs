//! The translation layer. Every piece of interface text goes through [`tr`]: a stable key and
//! the English text. The English text is the fallback, so a missing translation never shows a
//! key. Translations live in `assets/locales/<code>.json` (flat key to text objects), embedded in
//! the program. `en.json` is generated from the code by a test and kept in step by it. The
//! languages and their files are listed in `languages.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
#[cfg(not(test))]
use std::sync::RwLock;

use crate::commands::Cmd;

/// The active language code (empty: English). The program has one interface, so one value.
#[cfg(not(test))]
static ACTIVE: RwLock<String> = RwLock::new(String::new());

// Tests run many apps side by side in one process, each on its own thread. With one shared
// value, a test that switches the language would change the text under every other test, so
// in test builds each thread has its own language.
#[cfg(test)]
thread_local! {
    static ACTIVE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

#[cfg(not(test))]
fn active() -> String {
    ACTIVE.read().map(|a| a.clone()).unwrap_or_default()
}

#[cfg(not(test))]
fn set_active(code: &str) {
    if let Ok(mut a) = ACTIVE.write()
        && *a != code
    {
        *a = code.to_string();
    }
}

#[cfg(test)]
fn active() -> String {
    ACTIVE.with(|a| a.borrow().clone())
}

#[cfg(test)]
fn set_active(code: &str) {
    ACTIVE.with(|a| *a.borrow_mut() = code.to_string());
}

fn tables() -> &'static HashMap<&'static str, HashMap<String, String>> {
    static TABLES: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();
    TABLES.get_or_init(|| {
        crate::languages::files()
            .iter()
            .map(|(code, text)| {
                let table: HashMap<String, String> = serde_json::from_str(text).unwrap_or_else(|e| {
                    log::error!("locale {code} is damaged: {e}");
                    HashMap::new()
                });
                (*code, table)
            })
            .collect()
    })
}

/// Whether language `code` can be chosen: it is in the Language menu.
pub fn available(code: &str) -> bool {
    crate::languages::is_offered(code)
}

/// Switch the interface language. An unknown code leaves English.
pub fn set_language(code: &str) {
    let code = if available(code) { code } else { "en" };
    set_active(code);
    // Captions in capitals follow their language: Turkish writes "BİRİMLER", not "BIRIMLER".
    darkburn_ui::casing::set_text_language(text_language);
}

/// The language `text` is written in: the active language when `text` is one of its
/// translations, English otherwise (a text not translated yet shows in English).
pub fn text_language(text: &str) -> String {
    let lang = language();
    if lang != "en" && translated_texts(&lang).is_some_and(|texts| texts.contains(text)) { lang } else { "en".into() }
}

/// Every translated text of language `code`.
fn translated_texts(code: &str) -> Option<&'static HashSet<String>> {
    static TEXTS: OnceLock<HashMap<&'static str, HashSet<String>>> = OnceLock::new();
    TEXTS.get_or_init(|| tables().iter().map(|(code, table)| (*code, table.values().cloned().collect())).collect()).get(code)
}

/// The language in use.
pub fn language() -> String {
    let code = active();
    if code.is_empty() { "en".into() } else { code }
}

/// The text for `key` in the active language; `english` when there is none.
pub fn tr(key: &str, english: &str) -> String {
    let lang = language();
    if lang == "en" {
        return english.to_string();
    }
    lookup(tables().get(lang.as_str()), key, english)
}

/// The text for `key` in `table`; `english` when the table, the key or the text is missing.
fn lookup(table: Option<&HashMap<String, String>>, key: &str, english: &str) -> String {
    match table.and_then(|t| t.get(key)) {
        Some(text) if !text.trim().is_empty() => text.clone(),
        _ => english.to_string(),
    }
}

/// A command's label in the active language.
pub fn cmd_label(c: &Cmd) -> String {
    tr(&format!("cmd.{}", c.id), c.label)
}

/// A panel's tab title.
pub fn panel_title(id: &str, english: &str) -> String {
    tr(&format!("panel.{id}"), english)
}

/// The `tr("key", "English")` calls in `source`, in order.
#[cfg(test)]
pub fn scan_calls(source: &str) -> Vec<(String, String)> {
    fn string(s: &str) -> Option<(String, &str)> {
        let s = s.strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = s.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &s[i + 1..])),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    }
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(i) = rest.find("tr(") {
        let before_ok = i == 0 || !rest[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let tail = rest[i + 3..].trim_start();
        rest = &rest[i + 3..];
        if !before_ok {
            continue;
        }
        if let Some((key, after)) = string(tail)
            && let Some(after) = after.trim_start().strip_prefix(',')
            && let Some((english, _)) = string(after.trim_start())
        {
            found.push((key, english));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Source files whose text goes through `tr`.
    const SOURCES: &[(&str, &str)] = &[
        ("settings.rs", include_str!("settings.rs")),
        ("shell.rs", include_str!("shell.rs")),
        ("ribbon.rs", include_str!("ribbon.rs")),
        ("panels.rs", include_str!("panels.rs")),
        ("library.rs", include_str!("library.rs")),
        ("art_library.rs", include_str!("art_library.rs")),
        ("keymap.rs", include_str!("keymap.rs")),
        ("help.rs", include_str!("help.rs")),
        ("extras.rs", include_str!("extras.rs")),
        ("updates.rs", include_str!("updates.rs")),
        ("cuts_panel.rs", include_str!("cuts_panel.rs")),
    ];

    /// The complete English table: literal calls plus the tables of commands, menus, panels,
    /// toolbars, ribbon tabs and groups, and languages.
    fn english() -> BTreeMap<String, String> {
        let mut t = BTreeMap::new();
        for (file, src) in SOURCES {
            for (k, e) in scan_calls(src) {
                if let Some(old) = t.insert(k.clone(), e.clone()) {
                    assert_eq!(old, e, "key {k} has two English texts (second in {file})");
                }
            }
        }
        for c in crate::commands::COMMANDS.iter().filter(|c| c.args.is_empty()) {
            t.insert(format!("cmd.{}", c.id), c.label.to_string());
        }
        for (name, entries) in crate::commands::MENUS {
            t.insert(format!("menu.{name}"), name.to_string());
            collect_subs(entries, &mut t);
        }
        for p in crate::panels::PANELS {
            t.insert(format!("panel.{}", p.id), p.title.to_string());
        }
        for c in crate::settings::CHOICES {
            t.insert(format!("choice.{c}"), c.to_string());
        }
        for (id, name) in crate::prefs::TOOLBARS.iter().copied().chain([("status", "Status Bar")]) {
            t.insert(format!("toolbar.{id}"), name.to_string());
        }
        for (id, label, captions) in crate::ribbon::tab_texts() {
            t.insert(format!("ribbon.tab.{id}"), label.to_string());
            for c in captions {
                t.insert(format!("ribbon.group.{c}"), c.to_string());
            }
        }
        t
    }

    fn collect_subs(entries: &[crate::commands::Entry], t: &mut BTreeMap<String, String>) {
        for e in entries {
            if let crate::commands::Entry::Sub(name, inner) = e {
                t.insert(format!("menu.{name}"), name.to_string());
                collect_subs(inner, t);
            }
        }
    }

    fn english_file() -> &'static str {
        crate::languages::file("en").expect("assets/locales/en.json")
    }

    fn locale_path(code: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/locales").join(format!("{code}.json"))
    }

    #[test]
    fn english_file_matches_the_code() {
        let want = english();
        if std::env::var_os("DARKBURN_WRITE_LOCALES").is_some() {
            std::fs::write(locale_path("en"), serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
        }
        let have: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        // Commands other builders added since are fine; every other key must match exactly.
        let strip = |m: &BTreeMap<String, String>| -> BTreeMap<String, String> {
            m.iter().filter(|(k, _)| !k.starts_with("cmd.")).map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        assert_eq!(strip(&have), strip(&want), "run the tests with DARKBURN_WRITE_LOCALES=1 to regenerate assets/locales/en.json");
        for (k, v) in &have {
            if let Some(w) = want.get(k) {
                assert_eq!(v, w, "{k}");
            }
        }
    }

    #[test]
    fn norwegian_covers_the_english_keys() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        let nb: BTreeMap<String, String> = serde_json::from_str(crate::languages::file("nb").unwrap()).unwrap();
        let missing: Vec<_> = en.keys().filter(|k| !k.starts_with("cmd.") && !nb.contains_key(*k)).collect();
        assert!(missing.is_empty(), "nb.json lacks: {missing:?}");
        let cmds = en.keys().filter(|k| k.starts_with("cmd.")).count();
        let done = en.keys().filter(|k| k.starts_with("cmd.") && nb.contains_key(*k)).count();
        assert!(done * 10 >= cmds * 9, "only {done} of {cmds} command labels are in Norwegian");
        for (k, v) in &nb {
            assert!(en.contains_key(k), "nb.json has {k}, English does not");
            assert!(!v.trim().is_empty(), "{k} is empty");
        }
    }

    /// Why translation file `t` is not fit to ship next to English `en`: a key English does not
    /// have, an empty text, or fewer than nine in ten of the English keys. A text the program
    /// gains later may be missing; it shows in English until somebody translates it, so adding
    /// a text in English and Norwegian only never breaks this check.
    fn translation_gaps(en: &BTreeMap<String, String>, t: &BTreeMap<String, String>) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in t {
            if !en.contains_key(k) {
                out.push(format!("has {k}, English does not"));
            }
            if v.trim().is_empty() {
                out.push(format!("{k} is empty"));
            }
        }
        let done = en.keys().filter(|k| t.contains_key(*k)).count();
        if done * 10 < en.len() * 9 {
            out.push(format!("only {done} of {} English texts are translated", en.len()));
        }
        out
    }

    fn locale(code: &str) -> BTreeMap<String, String> {
        serde_json::from_str(crate::languages::file(code).unwrap()).unwrap()
    }

    /// German, French and Italian translate the English texts, and the Language menu offers
    /// them [I18N-04].
    #[test]
    fn german_french_and_italian_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("de", "Datei"), ("fr", "Fichier"), ("it", "File")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Spanish for Spain, Spanish for Latin America and (Brazilian) Portuguese translate the
    /// English texts, and the Language menu offers them [I18N-04].
    #[test]
    fn spanish_and_portuguese_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("es-ES", "Archivo"), ("es-LA", "Archivo"), ("pt", "Arquivo")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Dutch, Danish and Swedish translate the English texts, and the Language menu offers
    /// them [I18N-04].
    #[test]
    fn dutch_danish_and_swedish_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("nl", "Bestand"), ("da", "Filer"), ("sv", "Arkiv")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Finnish, Polish and Czech translate the English texts, and the Language menu offers
    /// them [I18N-04].
    #[test]
    fn finnish_polish_and_czech_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("fi", "Tiedosto"), ("pl", "Plik"), ("cs", "Soubor")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Hungarian, Slovenian and Turkish translate the English texts, and the Language menu
    /// offers them [I18N-04].
    #[test]
    fn hungarian_slovenian_and_turkish_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("hu", "Fájl"), ("sl", "Datoteka"), ("tr", "Dosya")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Greek and Russian translate the English texts, and the Language menu offers them
    /// [I18N-04].
    #[test]
    fn greek_and_russian_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("el", "Αρχείο"), ("ru", "Файл")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Greek asks with ";" (the Greek question mark), never "?", and both Greek and Russian
    /// quote the English labels the help names with « », not straight quotes.
    #[test]
    fn greek_and_russian_keep_their_own_punctuation() {
        for (code, text) in locale("el") {
            assert!(!text.contains('?'), "el {code}: {text:?}");
        }
        for lang in ["el", "ru"] {
            for (key, text) in locale(lang) {
                if key != "library.thickness_hint" {
                    assert!(!text.contains('"'), "{lang} {key}: {text:?}");
                }
            }
            let help = &locale(lang)["help.camera_selection.p1"];
            for label in ["Allow live camera", "Find cameras", "Camera", "Start"] {
                assert!(help.contains(&format!("\u{ab}{label}\u{bb}")), "{lang}: {help:?}");
            }
        }
    }

    /// Japanese and Korean translate the English texts, and the Language menu offers them
    /// [I18N-04].
    #[test]
    fn japanese_and_korean_are_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("ja", "ファイル"), ("ko", "파일")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Japanese quotes the English labels the help names with 「」 and Korean with ' ', never
    /// straight double quotes; the Start button keeps its English name in both.
    #[test]
    fn japanese_and_korean_keep_their_own_punctuation() {
        for (lang, open, close) in [("ja", "\u{300c}", "\u{300d}"), ("ko", "'", "'")] {
            for (key, text) in locale(lang) {
                if key != "library.thickness_hint" {
                    assert!(!text.contains('"'), "{lang} {key}: {text:?}");
                }
            }
            let help = &locale(lang)["help.camera_selection.p1"];
            for label in ["Allow live camera", "Find cameras", "Camera", "Start"] {
                assert!(help.contains(&format!("{open}{label}{close}")), "{lang}: {help:?}");
            }
        }
    }

    /// Review corrections: Danish aligns "nederst"/"øverst"/"midt" (not the plurals "bunde",
    /// "toppe", "midter"), the library button puts settings *on* a layer ("til lag", not
    /// "assign a layer"), and the Move help says "kør hjem".
    #[test]
    fn danish_and_swedish_keep_the_review_wording() {
        let da = locale("da");
        let sv = locale("sv");
        for (key, want) in [
            ("cmd.arrange.align_bottom", "Juster nederst"),
            ("cmd.arrange.align_top", "Juster øverst"),
            ("cmd.arrange.align_centres", "Juster midt"),
            ("cmd.arrange.align_h_centres", "Juster midt vandret"),
            ("cmd.arrange.align_v_centres", "Juster midt lodret"),
            ("cmd.arrange.distribute_h_centres", "Fordel midt vandret"),
            ("cmd.arrange.distribute_v_centres", "Fordel midt lodret"),
            ("library.assign", "Tildel til lag"),
        ] {
            assert_eq!(da[key], want, "da {key}");
        }
        assert!(da["help.move.p1"].contains("kør hjem"), "da help.move.p1: {:?}", da["help.move.p1"]);
        assert!(!da["help.move.p1"].contains("til hjem"), "da help.move.p1: {:?}", da["help.move.p1"]);
        assert_eq!(sv["library.assign"], "Tilldela till lager");
    }

    /// Review corrections for Finnish, Polish and Czech: Czech Nest packs shapes ("naskládat",
    /// not "vnořit", put inside) and its hotkey conflict lines make the key the subject
    /// ("Ctrl+S je už přiřazena příkazu ..."); a linked Polish library entry (wpis) is
    /// masculine; Finnish Dock pushes a shape ("työnnä"; "telakoida" is docking a window) and
    /// layer output is "ajo", never "tulostus", which is Print.
    #[test]
    fn finnish_polish_and_czech_keep_the_review_wording() {
        let fi = locale("fi");
        let pl = locale("pl");
        let cs = locale("cs");
        assert_eq!(cs["cmd.arrange.nest"], "Naskládat vybrané");
        assert_eq!(cs["hotkeys.taken_by"], "je už přiřazena příkazu");
        assert_eq!(cs["hotkeys.clash"], "přiřazena více příkazům");
        assert!(cs.values().all(|v| !v.to_lowercase().contains("vnoř")), "cs: a text still says vnořit");
        assert_eq!(pl["library.linked"], "połączony");
        for (key, want) in [
            ("cmd.arrange.dock_left", "Työnnä vasemmalle"),
            ("cmd.arrange.dock_right", "Työnnä oikealle"),
            ("cmd.arrange.dock_up", "Työnnä ylös"),
            ("cmd.arrange.dock_down", "Työnnä alas"),
            ("menu.Dock", "Työnnä"),
            ("ribbon.group.Docking", "Työntö"),
            ("toolbar.docking", "Työntö"),
            ("settings.output_tolerance", "Ajon käyrätoleranssi"),
        ] {
            assert_eq!(fi[key], want, "fi {key}");
        }
        assert!(fi.values().all(|v| !v.to_lowercase().contains("telak")), "fi: a text still says telakoida");
        // Output is never the Print word, and the help names the column by its header.
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (key, english) in &en {
            let lower = english.to_lowercase();
            if lower.contains("output") && !lower.contains("print") {
                assert!(!fi[key].to_lowercase().contains("tulost"), "fi {key} uses the Print word: {:?}", fi[key]);
            }
        }
        assert!(fi["help.cuts.p2"].contains(&format!("({})", fi["cuts.col.out"])), "fi help.cuts.p2: {:?}", fi["help.cuts.p2"]);
    }

    /// A Turkish text is Turkish, and a text with no Turkish translation yet is still English,
    /// so its capitals stay English ("SETTINGS", not "SETTİNGS").
    #[test]
    fn a_text_is_in_the_language_it_is_written_in() {
        set_language("tr");
        assert_eq!(text_language("Birimler"), "tr");
        assert_eq!(text_language("Sesler ve iş günlüğü"), "tr");
        assert_eq!(text_language("Layer 00 settings"), "en");
        assert_eq!(darkburn_ui::casing::upper("Birimler"), "BİRİMLER");
        assert_eq!(darkburn_ui::casing::upper("Layer 00 settings"), "LAYER 00 SETTINGS");
        set_language("en");
        assert_eq!(text_language("Birimler"), "en");
        assert_eq!(darkburn_ui::casing::upper("Units"), "UNITS");
    }

    /// Review corrections for Hungarian, Slovenian and Turkish: the hotkey conflict lines make
    /// the key the subject ("Ctrl+S je že dodeljena ukazu ...", "Ctrl+S már foglalt, ez a
    /// parancs használja ..."), not the command or an order to the user; the Slovenian size and
    /// quality choices agree with "Ikone" and "kakovost"; Turkish Flatten Array is not the
    /// Distribute verb "dağıt", and the art library tip says "birden fazlası".
    #[test]
    fn hungarian_slovenian_and_turkish_keep_the_review_wording() {
        let hu = locale("hu");
        let sl = locale("sl");
        let tr = locale("tr");
        assert_eq!(sl["hotkeys.taken_by"], "je že dodeljena ukazu");
        assert_eq!(sl["hotkeys.clash"], "dodeljena več ukazom");
        assert_eq!(hu["hotkeys.taken_by"], "már foglalt, ez a parancs használja");
        for (key, want) in [
            ("choice.Small", "Majhne"),
            ("choice.Medium", "Srednje"),
            ("choice.Large", "Velike"),
            ("choice.Coarse", "Groba"),
            ("choice.Normal", "Običajna"),
            ("choice.Fine", "Fina"),
        ] {
            assert_eq!(sl[key], want, "sl {key}");
        }
        assert_eq!(tr["cmd.arrange.array_flatten"], "Diziyi düzleştir");
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (key, text) in &tr {
            if text.to_lowercase().contains("dağıt") {
                assert!(en[key].to_lowercase().contains("distribut"), "tr {key} uses the Distribute word: {text:?}");
            }
        }
        assert!(!tr["art.load_tip"].contains("birden çoğu"), "tr art.load_tip: {:?}", tr["art.load_tip"]);
    }

    /// Review corrections for Greek and Russian: the shared Dark/Light choices are feminine in
    /// Russian (they agree with "Тема"), so the canvas Background row takes a feminine noun too;
    /// the Russian visibility column is not "Видим" ("we see"); Flatten Image Crop says the
    /// crop becomes permanent and reads differently from Apply Crop Path; the Greek debug log
    /// is "αποσφαλμάτωση" (debugging), not a log of errors only.
    #[test]
    fn greek_and_russian_keep_the_review_wording() {
        let el = locale("el");
        let ru = locale("ru");
        assert_eq!(ru["choice.Dark"], "Темная");
        assert_eq!(ru["choice.Light"], "Светлая");
        assert_eq!(ru["settings.background"], "Подложка");
        assert_eq!(ru["cuts.col.show"], "Показ");
        assert!(ru.values().all(|v| !v.split(|c: char| !c.is_alphabetic()).any(|w| w == "Видим")), "ru: a text still says Видим");
        assert_eq!(ru["cmd.edit.image_flatten"], "Окончательно обрезать изображение");
        assert!(!ru["cmd.edit.image_flatten"].starts_with("Применить"), "ru: Flatten reads like Apply Crop Path");
        assert_eq!(el["cmd.help.debug_log"], "Ενεργοποίηση καταγραφής αποσφαλμάτωσης");
        assert_eq!(el["help.debug_on"], "Η καταγραφή αποσφαλμάτωσης είναι ενεργή.");
        assert_eq!(el["help.debug_off"], "Η καταγραφή αποσφαλμάτωσης είναι ανενεργή.");
        for key in ["cmd.help.debug_log", "help.debug_on", "help.debug_off"] {
            assert!(!el[key].contains("σφαλμάτων"), "el {key}: {:?}", el[key]);
        }
    }

    /// The two Spanish files are not copies of each other: each uses the words of its own
    /// region, such as "Edición" and "Ajustes" in Spain against "Editar" and "Configuración"
    /// in Latin America, and "ordenador" against "computadora".
    #[test]
    fn spanish_for_spain_and_for_latin_america_use_their_own_words() {
        let spain = locale("es-ES");
        let latam = locale("es-LA");
        let differ = spain.iter().filter(|(k, v)| latam.get(*k) != Some(*v)).count();
        assert!(differ >= 30, "only {differ} texts differ between es-ES and es-LA");
        for (key, es, la) in [
            ("menu.Edit", "Edición", "Editar"),
            ("cmd.edit.settings", "Ajustes", "Configuración"),
            ("settings.invert_wheel", "ratón", "mouse"),
            ("help.hotkeys.p2", "ordenador", "computadora"),
        ] {
            assert!(spain[key].contains(es), "es-ES {key}: {:?}", spain[key]);
            assert!(latam[key].contains(la), "es-LA {key}: {:?}", latam[key]);
            assert!(!latam[key].contains(es), "es-LA {key} keeps the Spain word: {:?}", latam[key]);
        }
    }

    /// A builder who adds a few texts in English and Norwegian only does not break the German,
    /// French, Italian, Spanish and Portuguese checks; the new texts show in English there.
    #[test]
    fn texts_added_in_english_only_keep_the_other_languages_passing() {
        let mut en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for i in 0..8 {
            en.insert(format!("settings.added_later_{i}"), format!("Added later {i}"));
        }
        for code in
            ["de", "fr", "it", "es-ES", "es-LA", "pt", "nl", "da", "sv", "fi", "pl", "cs", "hu", "sl", "tr", "el", "ru", "ja", "ko", "zh-CN", "zh-TW"]
        {
            assert_eq!(translation_gaps(&en, &locale(code)), Vec::<String>::new(), "{code}");
        }
        // The check still catches a file that is mostly missing, a key English lacks and an empty text.
        let half: BTreeMap<String, String> = locale("de").into_iter().step_by(2).collect();
        assert!(!translation_gaps(&en, &half).is_empty());
        let mut odd = locale("de");
        odd.insert("no.such.key".into(), "X".into());
        odd.insert("menu.File".into(), " ".into());
        let gaps = translation_gaps(&en, &odd);
        assert!(gaps.iter().any(|g| g.contains("no.such.key")) && gaps.iter().any(|g| g.contains("menu.File is empty")), "{gaps:?}");
    }

    /// Two different English words seen side by side keep two different translations: the Edit
    /// menu (clipboard, undo) and the Modify ribbon tab (changing shapes) [I18N-04].
    #[test]
    fn edit_and_modify_are_different_words_in_every_language() {
        for (code, text) in crate::languages::files() {
            let t: BTreeMap<String, String> = serde_json::from_str(text).unwrap();
            if let (Some(edit), Some(modify)) = (t.get("menu.Edit"), t.get("ribbon.tab.modify")) {
                assert_ne!(edit, modify, "{code}: Edit and Modify are both {edit:?}");
            }
        }
    }

    /// The output column of the cut table holds tick boxes; a header that reads as "Off" makes a
    /// ticked layer look switched off.
    #[test]
    fn the_output_column_header_does_not_read_as_off() {
        let off = [
            ("de", "Aus"),
            ("nb", "Av"),
            ("fr", "Arrêt"),
            ("it", "Spento"),
            ("es-ES", "Apagado"),
            ("es-LA", "Apagado"),
            ("pt", "Desligado"),
            ("nl", "Uit"),
            ("da", "Fra"),
            ("sv", "Av"),
            ("fi", "Pois"),
            ("pl", "Wył"),
            ("cs", "Vyp"),
            ("hu", "Ki"),
            ("sl", "Izklop"),
            ("tr", "Kapalı"),
            ("el", "Ανενεργό"),
            ("ru", "Выкл"),
            ("ja", "オフ"),
            ("ko", "끔"),
            ("zh-CN", "关"),
            ("zh-TW", "關"),
            ("en", "Off"),
        ];
        for (code, word) in off {
            set_language(code);
            assert_ne!(tr("cuts.col.out", "Out"), word, "{code}");
        }
        set_language("en");
    }

    /// French keeps a no-break space before : ; ? ! and inside « »: with a normal space the
    /// punctuation can wrap onto a line of its own.
    #[test]
    fn french_punctuation_stays_on_the_line_of_its_word() {
        let mut bad = Vec::new();
        for (k, v) in locale("fr") {
            let chars: Vec<char> = v.chars().collect();
            for (i, c) in chars.iter().enumerate() {
                let space_before = i > 0 && chars[i - 1] == ' ';
                let space_after = chars.get(i + 1) == Some(&' ');
                if (matches!(c, ':' | ';' | '?' | '!' | '\u{bb}') && space_before) || (*c == '\u{ab}' && space_after) {
                    bad.push(format!("{k}: {v}"));
                    break;
                }
            }
        }
        assert!(bad.is_empty(), "normal space where a no-break space belongs:\n{}", bad.join("\n"));
    }

    /// Corrections from the review of the German, French and Italian files: words that agree
    /// with the label they belong to, and words that mean what the button does [I18N-04].
    #[test]
    fn reviewed_translations_keep_their_corrections() {
        let want = [
            // "Toolbar icons" is plural in French and Italian; its size choices agree with it.
            ("fr", "choice.Small", "Petites"),
            ("fr", "choice.Medium", "Moyennes"),
            ("fr", "choice.Large", "Grandes"),
            ("it", "choice.Small", "Piccole"),
            ("it", "choice.Medium", "Medie"),
            ("it", "choice.Large", "Grandi"),
            // The key is taken from another command, not taken back.
            ("fr", "hotkeys.take", "Prendre la touche"),
            ("de", "cuts.col.out", "Ausg."),
            ("it", "ribbon.tab.modify", "Trasforma"),
        ];
        for (code, key, text) in want {
            assert_eq!(locale(code).get(key).map(String::as_str), Some(text), "{code} {key}");
        }
        assert!(!locale("fr")["help.hotkeys.p1"].contains("reprendre"));
    }

    /// The laser panel's Start button is not translated yet, so the setting that names it says
    /// "Start" in every language; a translated word would point at a button that is not there.
    #[test]
    fn the_ignore_start_setting_names_the_button_as_it_reads() {
        for (code, text) in crate::languages::files() {
            let t: BTreeMap<String, String> = serde_json::from_str(text).unwrap();
            if let Some(label) = t.get("settings.ignore_start") {
                assert!(label.contains("Start"), "{code}: {label:?}");
            }
        }
    }

    /// One English term keeps one word throughout a Spanish or Portuguese file (see the
    /// glossary): Width, Height, Reset and a backup copy of a file. A backup path, the outline
    /// an image keeps, is a sense of its own.
    #[test]
    fn spanish_and_portuguese_use_one_word_per_term() {
        assert_one_word_per_term(&[
            ("es-ES", ["ancho", "alto", "restablecer", "copia de seguridad", "trazado de respaldo"]),
            ("es-LA", ["ancho", "alto", "restablecer", "respaldo", "trazado de respaldo"]),
            ("pt", ["largura", "altura", "restaurar", "backup", "caminho de backup"]),
        ]);
    }

    /// Dutch, Danish and Swedish keep one word per term the same way (see the glossary).
    #[test]
    fn dutch_danish_and_swedish_use_one_word_per_term() {
        assert_one_word_per_term(&[
            ("nl", ["breedte", "hoogte", "herstellen", "back-up", "reservepad"]),
            ("da", ["bredde", "højde", "nulstil", "sikkerhedskopi", "reservesti"]),
            ("sv", ["bredd", "höjd", "återställ", "säkerhetskopia", "reservbana"]),
        ]);
    }

    /// Finnish keeps one word per term like the languages above. Polish and Czech change the
    /// ending of a word with its place in the sentence ("kopia zapasowa", "kopię zapasową"), so
    /// their terms are given as stems: each word of the term starts a word of the text.
    #[test]
    fn finnish_polish_and_czech_use_one_word_per_term() {
        assert_one_word_per_term(&[("fi", ["leveys", "korkeus", "nollaa", "varmuuskopio", "varapolku"])]);
        assert_one_stem_per_term(&[
            ("pl", ["szerokość", "wysokość", "resetuj", "kopi zapasow", "ścieżk zapasow"]),
            ("cs", ["šířka", "výška", "resetovat", "záloh", "záložní cest"]),
        ]);
    }

    /// Hungarian, Slovenian and Turkish add endings to their words ("biztonsági mentésből",
    /// "varnostne kopije", "Genişlikleri"), so their terms are given as stems too.
    #[test]
    fn hungarian_slovenian_and_turkish_use_one_word_per_term() {
        assert_one_stem_per_term(&[
            ("hu", ["szélesség", "magasság", "visszaállít", "biztonsági mentés", "tartalék útvonal"]),
            ("sl", ["širin", "višin", "ponastavi", "varnostn kopij", "rezervn pot"]),
            ("tr", ["genişlik", "yükseklik", "sıfırla", "yedek", "yedek yol"]),
        ]);
    }

    /// Greek and Russian change the endings of their words too ("αντίγραφο ασφαλείας",
    /// "резервную копию"), so their terms are given as stems.
    #[test]
    fn greek_and_russian_use_one_word_per_term() {
        assert_one_stem_per_term(&[
            ("el", ["πλάτ", "ύψ", "επαναφορ", "αντίγραφ ασφαλείας", "εφεδρικ διαδρομ"]),
            ("ru", ["ширин", "высот", "сброс", "резервн копи", "запасн контур"]),
        ]);
    }

    /// Japanese writes no spaces between words, so its terms are found anywhere in the text.
    /// Korean attaches its particles to the word ("백업으로"), so its terms are given as stems.
    #[test]
    fn japanese_and_korean_use_one_word_per_term() {
        assert_terms(&[("ja", ["幅", "高さ", "リセット", "バックアップ", "予備パス"])], |text, w| text.contains(w));
        assert_one_stem_per_term(&[("ko", ["너비", "높이", "초기화", "백업", "백업 경로"])]);
    }

    /// Every text that names the selection says 選択範囲 in Japanese and 선택 영역 in Korean, so
    /// the help names the art library buttons as they read. Choosing a camera is another sense,
    /// and "Clear Selection" and "No selection" are the usual deselect phrases 選択を解除 /
    /// 選択なし and 선택 해제 / 선택 없음 [review of I18N-04].
    #[test]
    fn japanese_and_korean_name_the_selection_with_one_word() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        let other_sense = ["cmd.help.camera", "cmd.selection.clear", "status.no_selection"];
        let keys: Vec<&String> = en
            .iter()
            .filter(|(k, _)| !other_sense.contains(&k.as_str()) && !k.starts_with("help.camera_selection."))
            .filter(|(_, v)| v.to_lowercase().split(|c: char| !c.is_alphabetic()).any(|w| w == "selection"))
            .map(|(k, _)| k)
            .collect();
        assert!(keys.len() >= 15, "{keys:?}");
        let (ja, ko) = (locale("ja"), locale("ko"));
        let mut bad = Vec::new();
        for key in keys {
            if !ja[key].contains("選択範囲") {
                bad.push(format!("ja {key}: {:?} lacks 選択範囲", ja[key]));
            }
            if !starts_words(&ko[key], "선택 영역") {
                bad.push(format!("ko {key}: {:?} lacks 선택 영역", ko[key]));
            }
        }
        bad.extend(ko.iter().filter(|(_, v)| v.contains("선택 항목")).map(|(k, v)| format!("ko {k}: {v:?} says 선택 항목")));
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Review corrections in Japanese and Korean: the job origin setting names the option
    /// "Cut selected graphics" (not the cut settings of a selection), and Apply Path to Text is
    /// a whole command name in Korean [review of I18N-04].
    #[test]
    fn japanese_and_korean_keep_the_review_wording() {
        let (ja, ko) = (locale("ja"), locale("ko"));
        assert_eq!(ja["settings.cut_selected"], "既定で選択したグラフィックをカット");
        assert_eq!(ja["settings.save_job_origin"], "ジョブ原点と「選択したグラフィックをカット」をプロジェクトに保存");
        assert_eq!(ko["settings.cut_selected"], "기본적으로 선택한 그래픽 절단");
        assert_eq!(ko["settings.save_job_origin"], "작업 원점과 선택한 그래픽 절단 여부를 프로젝트에 저장");
        assert_eq!(ko["cmd.tools.path_to_text"], "텍스트에 경로 적용");
    }

    /// Simplified (mainland) and Traditional (Taiwan) Chinese translate the English texts, and
    /// the Language menu offers both [I18N-04].
    #[test]
    fn chinese_is_translated_and_offered() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        for (code, file) in [("zh-CN", "文件"), ("zh-TW", "檔案")] {
            let gaps = translation_gaps(&en, &locale(code));
            assert!(gaps.is_empty(), "{code}.json: {gaps:?}");
            assert!(crate::languages::is_offered(code), "{code} is not in the Language menu");
            set_language(code);
            assert_eq!(tr("menu.File", "File"), file, "{code}");
        }
        set_language("en");
    }

    /// Taiwan Chinese is not Simplified Chinese with converted characters: it uses Taiwan's own
    /// software words (檔案, 儲存, 雷射, 復原 ...), and neither file mixes in characters of the
    /// other script.
    #[test]
    fn traditional_chinese_uses_taiwan_words_not_converted_mainland_ones() {
        let (cn, tw) = (locale("zh-CN"), locale("zh-TW"));
        for (key, mainland, taiwan) in [
            ("menu.File", "文件", "檔案"),
            ("cmd.file.save", "保存", "儲存"),
            ("panel.laser", "激光", "雷射"),
            ("cmd.edit.undo", "撤销", "復原"),
            ("cmd.edit.paste", "粘贴", "貼上"),
            ("cmd.edit.cut", "剪切", "剪下"),
            ("help.title", "帮助", "說明"),
            ("menu.Window", "窗口", "視窗"),
            ("menu.toolbars", "工具栏", "工具列"),
            ("cmd.file.hotkeys", "快捷键", "快速鍵"),
            ("panel.camera", "摄像头", "攝影機"),
            ("cmd.file.prefs_import", "导入", "匯入"),
            ("library.material", "材料", "材質"),
            ("settings.page.general", "常规", "一般"),
        ] {
            assert!(cn[key].contains(mainland), "zh-CN {key}: {:?} lacks {mainland}", cn[key]);
            assert!(tw[key].contains(taiwan), "zh-TW {key}: {:?} lacks {taiwan}", tw[key]);
        }
        let simplified_only = "们个这设图选删开关软档轮钮单贴导择编辑键";
        let traditional_only = "們個這設圖選刪開關軟檔輪鈕單貼導擇編輯鍵";
        let mut bad = Vec::new();
        for (code, t, foreign) in [("zh-CN", &cn, traditional_only), ("zh-TW", &tw, simplified_only)] {
            for (key, text) in t.iter() {
                if text.chars().any(|c| foreign.contains(c)) {
                    bad.push(format!("{code} {key}: {text:?}"));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
        let differ = cn.iter().filter(|(k, v)| tw.get(*k) != Some(v)).count();
        assert!(differ * 10 >= cn.len() * 8, "only {differ} of {} texts differ", cn.len());
    }

    /// Simplified Chinese quotes the English labels the help names with “ ” and Traditional
    /// Chinese with 「」, never straight double quotes; the Start button keeps its English name.
    #[test]
    fn chinese_keeps_its_own_punctuation() {
        for (lang, open, close) in [("zh-CN", "\u{201c}", "\u{201d}"), ("zh-TW", "\u{300c}", "\u{300d}")] {
            for (key, text) in locale(lang) {
                if key != "library.thickness_hint" {
                    assert!(!text.contains('"'), "{lang} {key}: {text:?}");
                    assert!(!text.contains('(') && !text.contains(')'), "{lang} {key}: half-width brackets in {text:?}");
                }
            }
            let help = &locale(lang)["help.camera_selection.p1"];
            for label in ["Allow live camera", "Find cameras", "Camera", "Start"] {
                assert!(help.contains(&format!("{open}{label}{close}")), "{lang}: {help:?}");
            }
        }
    }

    /// Chinese writes no spaces between words, so its terms are found anywhere in the text.
    #[test]
    fn chinese_uses_one_word_per_term() {
        assert_terms(
            &[("zh-CN", ["宽度", "高度", "重置", "备份", "备用路径"]), ("zh-TW", ["寬度", "高度", "重設", "備份", "備用路徑"])],
            |text, w| text.contains(w),
        );
    }

    /// Every text that names the selection says 选区 in Simplified and 選取範圍 in Traditional
    /// Chinese. Invert Selection keeps the usual idiom 反选 / 反向選取, and choosing a camera,
    /// "Clear Selection" and "No selection" are other senses [I18N-04].
    #[test]
    fn chinese_names_the_selection_with_one_word() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        let other_sense = ["cmd.help.camera", "cmd.selection.clear", "status.no_selection", "cmd.edit.invert_selection"];
        let keys: Vec<&String> = en
            .iter()
            .filter(|(k, _)| !other_sense.contains(&k.as_str()) && !k.starts_with("help.camera_selection."))
            .filter(|(_, v)| v.to_lowercase().split(|c: char| !c.is_alphabetic()).any(|w| w == "selection"))
            .map(|(k, _)| k)
            .collect();
        assert!(keys.len() >= 14, "{keys:?}");
        let mut bad = Vec::new();
        for (code, word) in [("zh-CN", "选区"), ("zh-TW", "選取範圍")] {
            let t = locale(code);
            bad.extend(keys.iter().filter(|k| !t[**k].contains(word)).map(|k| format!("{code} {k}: {:?} lacks {word}", t[*k])));
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The job origin setting names the option "Cut selected graphics" in both Chinese files, as
    /// Japanese and Korean do.
    #[test]
    fn chinese_names_the_cut_selected_option_as_it_reads() {
        let (cn, tw) = (locale("zh-CN"), locale("zh-TW"));
        assert_eq!(cn["settings.cut_selected"], "默认仅切割所选图形");
        assert_eq!(cn["settings.save_job_origin"], "在项目中保存作业原点和\u{201c}仅切割所选图形\u{201d}");
        assert_eq!(tw["settings.cut_selected"], "預設僅切割選取的圖形");
        assert_eq!(tw["settings.save_job_origin"], "在專案中儲存工作原點與\u{300c}僅切割選取的圖形\u{300d}");
    }

    /// Review corrections in Chinese [review of I18N-04]: the Library panel's tab, the label
    /// inside it, its help and the cut library commands name one library (切割库 /
    /// 切割庫); every Remove command says 移除 and every Delete command the Delete word, so the
    /// two never read alike; and the getting-started help quotes the Settings button and the
    /// Edit Hotkeys command by their names instead of running them into one phrase.
    #[test]
    fn chinese_keeps_the_review_wording() {
        let en: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        let commands = |word: &str| -> Vec<String> {
            en.iter().filter(|(k, v)| k.starts_with("cmd.") && v.split(' ').next() == Some(word)).map(|(k, _)| k.clone()).collect()
        };
        let (removes, deletes) = (commands("Remove"), commands("Delete"));
        assert!(removes.len() >= 5 && deletes.len() >= 3, "{removes:?} {deletes:?}");
        let mut bad = Vec::new();
        for (code, library, delete, open, close) in
            [("zh-CN", "切割库", "删除", "\u{201c}", "\u{201d}"), ("zh-TW", "切割庫", "刪除", "\u{300c}", "\u{300d}")]
        {
            let t = locale(code);
            for key in ["panel.library", "library.name", "cmd.library.new", "help.library.title", "help.library.p1", "help.library.p2"] {
                if !t[key].contains(library) {
                    bad.push(format!("{code} {key}: {:?} lacks {library}", t[key]));
                }
            }
            for key in &removes {
                if !t[key].contains("移除") || t[key].contains(delete) {
                    bad.push(format!("{code} {key}: {:?} is not 移除", t[key]));
                }
            }
            for key in &deletes {
                if !t[key].contains(delete) {
                    bad.push(format!("{code} {key}: {:?} lacks {delete}", t[key]));
                }
            }
            let help = &t["help.general.p2"];
            for name in ["cmd.edit.settings", "cmd.file.hotkeys"] {
                if !help.contains(&format!("{open}{}{close}", t[name])) {
                    bad.push(format!("{code} help.general.p2: {help:?} does not quote {:?}", t[name]));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// The stem check finds a term in any of its forms, and only at the start of words.
    #[test]
    fn the_stem_check_matches_the_start_of_each_word() {
        assert!(starts_words("wczytaj kopię zapasową preferencji", "kopi zapasow"));
        assert!(starts_words("stary plik zostaje jako kopia zapasowa", "kopi zapasow"));
        assert!(!starts_words("kopiuj plik zapasowy", "kopi zapasow"));
        assert!(!starts_words("skopiuj zapasową", "kopi zapasow"));
        assert!(!starts_words("anything", ""));
    }

    /// Each language's words for Width, Height, Reset, a backup copy and a backup path appear,
    /// as whole words, in every text that names that term.
    fn assert_one_word_per_term(words: &[(&str, [&str; 5])]) {
        assert_terms(words, |lower, w| {
            // "alto" must not match inside "altura": compare whole words.
            lower.match_indices(w).any(|(i, _)| {
                let before = lower[..i].chars().next_back().is_none_or(|c| !c.is_alphabetic());
                let after = lower[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphabetic());
                before && after
            })
        });
    }

    /// As [`assert_one_word_per_term`], for languages whose words change their endings.
    fn assert_one_stem_per_term(stems: &[(&str, [&str; 5])]) {
        assert_terms(stems, starts_words);
    }

    /// True when the words of `stem` start consecutive words of `lower`.
    fn starts_words(lower: &str, stem: &str) -> bool {
        let words: Vec<&str> = lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).collect();
        let stems: Vec<&str> = stem.split_whitespace().collect();
        !stems.is_empty() && words.windows(stems.len()).any(|run| run.iter().zip(&stems).all(|(w, s)| w.starts_with(s)))
    }

    fn assert_terms(words: &[(&str, [&str; 5])], found: impl Fn(&str, &str) -> bool) {
        let width = ["settings.width", "settings.capture_width", "cmd.arrange.same_width"];
        let height = ["settings.height", "settings.capture_height", "settings.font_height", "cmd.arrange.same_height"];
        let reset = ["cmd.laser.pnc_reset", "cmd.window.reset_layout", "hotkeys.reset_all", "settings.reset_all"];
        let backup = ["cmd.file.prefs_backup", "library.save_tip"];
        let path = ["cmd.edit.image_remove_backup"];
        let mut bad = Vec::new();
        for (code, word) in words {
            let t = locale(code);
            for (keys, w) in [(&width[..], word[0]), (&height[..], word[1]), (&reset[..], word[2]), (&backup[..], word[3]), (&path[..], word[4])] {
                for key in keys {
                    let text = &t[*key];
                    if !found(&text.to_lowercase(), w) {
                        bad.push(format!("{code} {key}: {text:?} lacks {w:?}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn translated_text_is_used_and_english_is_the_fallback() {
        // Each test thread has its own language (see `ACTIVE`), so switching it here does not
        // change the text under other tests.
        set_language("nb");
        assert_eq!(tr("menu.File", "File"), "Fil");
        assert_eq!(tr("no.such.key", "Fallback"), "Fallback");
        set_language("xx");
        assert_eq!(language(), "en");
        assert_eq!(tr("menu.File", "File"), "File");
    }

    #[test]
    fn ui_text_does_not_bypass_the_translation_layer() {
        // Widgets whose first text argument is a bare literal skip `tr`.
        let widgets =
            ["ui_kit::row(ui, \"", "ui_kit::checkbox(ui, \"", "ui_kit::section_heading(ui, \"", "ui_kit::caption(ui, \"", "ui_kit::button(ui, \""];
        for (file, src) in SOURCES {
            if !matches!(*file, "settings.rs" | "library.rs" | "art_library.rs" | "extras.rs" | "help.rs" | "keymap.rs") {
                continue;
            }
            for w in widgets {
                assert!(!src.contains(w), "{file} passes a literal to {w}...: wrap it in tr()");
            }
        }
    }

    #[test]
    fn a_key_the_file_lacks_or_leaves_empty_shows_the_english_text() {
        // A language whose file has only a few keys yet, as a new one starts out.
        let partial: HashMap<String, String> = [("menu.File", "Datei"), ("menu.Edit", " ")].into_iter().map(|(k, v)| (k.into(), v.into())).collect();
        assert_eq!(lookup(Some(&partial), "menu.File", "File"), "Datei");
        assert_eq!(lookup(Some(&partial), "menu.Tools", "Tools"), "Tools");
        assert_eq!(lookup(Some(&partial), "menu.Edit", "Edit"), "Edit");
        assert_eq!(lookup(None, "menu.File", "File"), "File");
    }

    #[test]
    fn every_language_in_the_menu_is_named_in_itself() {
        assert_eq!(crate::languages::find("nb").map(|l| l.name), Some("Norsk bokm\u{e5}l"));
        assert!(available("nb") && available("en"));
        assert!(!available("xx"));
        // A language in the table without its file cannot be chosen; it falls back to English.
        if let Some(l) = crate::languages::LANGUAGES.iter().find(|l| crate::languages::file(l.code).is_none()) {
            assert!(!available(l.code));
            set_language(l.code);
            assert_eq!(language(), "en");
        }
    }

    // ---- The rules every translation file keeps [I18N-04] ----

    /// The `{name}` placeholders of `text` (`{}` included), sorted. The program fills them in, so
    /// a translation has the same ones, in any order.
    fn placeholders(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(i) = rest.find('{') {
            rest = &rest[i + 1..];
            if let Some(end) = rest.find('}') {
                let name = &rest[..end];
                if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push(format!("{{{name}}}"));
                    rest = &rest[end + 1..];
                }
            }
        }
        out.sort();
        out
    }

    /// How many shortcut marks `text` has: `&` right before a letter (an accelerator; `&&` and
    /// `& ` are a plain ampersand).
    fn accelerators(text: &str) -> usize {
        let chars: Vec<char> = text.chars().collect();
        let mut n = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '&' {
                match chars.get(i + 1) {
                    Some('&') => i += 1,
                    Some(c) if !c.is_whitespace() => n += 1,
                    _ => {}
                }
            }
            i += 1;
        }
        n
    }

    /// The key names in `text`, sorted. They name keys on the keyboard, so they stay as they are.
    /// Ctrl, Cmd, Esc and F1 to F24 count wherever they stand as a word. Shift and Alt count only
    /// in a key combination, next to a `+` (`Alt+Drag`, `Ctrl+Shift+Z`): on their own they are
    /// ordinary words in some languages ("Alt" is "all" in Norwegian and Danish).
    fn key_names(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut words = Vec::new();
        let mut start = None;
        for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
            match (c.is_alphanumeric(), start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    words.push((s, i));
                    start = None;
                }
                _ => {}
            }
        }
        for (s, e) in words {
            let w = &text[s..e];
            let in_combination = text[..s].ends_with('+') || text[e..].starts_with('+');
            let key = match w {
                "Ctrl" | "Cmd" | "Esc" => true,
                "Shift" | "Alt" => in_combination,
                _ => w.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n) && !w.starts_with("F0")),
            };
            if key {
                out.push(w.to_string());
            }
        }
        out.sort();
        out
    }

    fn ends_with_colon(text: &str) -> bool {
        let t = text.trim_end();
        t.ends_with(':') || t.ends_with('\u{ff1a}')
    }

    fn ends_with_ellipsis(text: &str) -> bool {
        let t = text.trim_end().trim_end_matches([':', '\u{ff1a}']);
        t.ends_with("...") || t.ends_with('\u{2026}')
    }

    fn leading_space(text: &str) -> &str {
        &text[..text.len() - text.trim_start().len()]
    }

    fn trailing_space(text: &str) -> &str {
        &text[text.trim_end().len()..]
    }

    /// What is wrong with `text` as the translation of `english`.
    fn check_text(english: &str, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        if text.trim().is_empty() {
            out.push("is empty (leave the key out to show English)".to_string());
            return out;
        }
        if placeholders(text) != placeholders(english) {
            out.push(format!("has the placeholders {:?}, English has {:?}", placeholders(text), placeholders(english)));
        }
        if leading_space(text) != leading_space(english) || trailing_space(text) != trailing_space(english) {
            out.push("does not start and end with the same spaces as English".to_string());
        }
        if ends_with_colon(text) != ends_with_colon(english) {
            out.push(format!("must {}end with a colon, as English", if ends_with_colon(english) { "" } else { "not " }));
        }
        if ends_with_ellipsis(text) != ends_with_ellipsis(english) {
            out.push(format!("must {}end with \"...\", as English", if ends_with_ellipsis(english) { "" } else { "not " }));
        }
        if accelerators(text) != accelerators(english) {
            out.push(format!("has {} shortcut marks (&), English has {}", accelerators(text), accelerators(english)));
        }
        if key_names(text) != key_names(english) {
            out.push(format!("names the keys {:?}, English names {:?}", key_names(text), key_names(english)));
        }
        out
    }

    /// What is wrong with the locale file `text`, checked against the English table.
    fn check_file(english: &BTreeMap<String, String>, text: &str) -> Vec<String> {
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return vec![format!("is not valid JSON: {e}")],
        };
        let Some(object) = value.as_object() else {
            return vec!["is not one object of key to text".to_string()];
        };
        let mut out = Vec::new();
        for (key, v) in object {
            let Some(v) = v.as_str() else {
                out.push(format!("{key}: is not a text"));
                continue;
            };
            let Some(en) = english.get(key) else {
                out.push(format!("{key}: English has no such key"));
                continue;
            };
            out.extend(check_text(en, v).into_iter().map(|p| format!("{key}: {p}")));
        }
        out
    }

    #[test]
    fn every_locale_file_keeps_the_rules_of_the_english_text() {
        let english: BTreeMap<String, String> = serde_json::from_str(english_file()).unwrap();
        let dir = locale_path("en").parent().unwrap().to_path_buf();
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let problems = check_file(&english, &text);
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            assert!(problems.is_empty(), "{name}:\n  {}", problems.join("\n  "));
            checked += 1;
        }
        assert!(checked >= 2, "en.json and nb.json at least");
    }

    #[test]
    fn the_locale_checker_finds_each_kind_of_mistake() {
        let english: BTreeMap<String, String> = [
            ("a", "Export..."),
            ("b", "Speed:"),
            ("c", "Saved {count} files"),
            ("d", "&Open"),
            ("e", "Press F1 or Ctrl+H"),
            ("f", " mm"),
            ("g", "All"),
            ("h", "Hold Alt+Shift to snap"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let one = |key: &str, text: &str| check_file(&english, &serde_json::json!({ key: text }).to_string());
        // Each rule kept: no problem.
        for (k, v) in [
            ("a", "Exportieren..."),
            ("a", "\u{30a8}\u{30af}\u{30b9}\u{30dd}\u{30fc}\u{30c8}\u{2026}"),
            ("b", "Hastighet:"),
            ("b", "\u{901f}\u{5ea6}\u{ff1a}"),
        ] {
            assert!(one(k, v).is_empty(), "{k} {v}: {:?}", one(k, v));
        }
        for (k, v) in [
            ("c", "{count} Dateien gespeichert"),
            ("d", "\u{00c5}&pne"),
            ("e", "Trykk F1 eller Ctrl+H"),
            ("f", " mm"),
            ("g", "Alt"),
            ("h", "Hold Alt+Shift for \u{e5} feste"),
        ] {
            assert!(one(k, v).is_empty(), "{k} {v}: {:?}", one(k, v));
        }
        // Each rule broken: a problem that says which.
        let broken = [
            ("a", "Exportieren", "\"...\""),
            ("a", "Exportieren...:", "colon"),
            ("b", "Hastighet", "colon"),
            ("c", "{anzahl} Dateien gespeichert", "placeholders"),
            ("c", "Dateien gespeichert", "placeholders"),
            ("d", "Open", "shortcut marks"),
            ("e", "Trykk F2 eller Ctrl+H", "keys"),
            ("e", "Trykk F1 eller Strg+H", "keys"),
            ("h", "Hold Alt for \u{e5} feste", "keys"),
            ("h", "Hold Alt+Skift for \u{e5} feste", "keys"),
            ("f", "mm", "spaces"),
            ("f", " mm ", "spaces"),
            ("b", "   ", "empty"),
            ("zz", "Hei", "no such key"),
        ];
        for (k, v, why) in broken {
            let p = one(k, v);
            assert!(p.iter().any(|p| p.contains(why)), "{k} {v:?}: expected a problem about {why}, got {p:?}");
        }
        assert!(check_file(&english, "{\"a\": \"Export...\",}")[0].contains("not valid JSON"));
        assert!(check_file(&english, "[\"Export...\"]")[0].contains("one object"));
        assert!(check_file(&english, "{\"a\": 3}")[0].contains("not a text"));
        // The English file passes its own rules.
        let all = serde_json::to_string(&english).unwrap();
        assert!(check_file(&english, &all).is_empty());
    }
}
