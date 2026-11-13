//! The translation layer. Every piece of interface text goes through [`tr`]: a stable key and
//! the English text. The English text is the fallback, so a missing translation never shows a
//! key. Translations live in `assets/locales/<code>.json` (flat key to text objects), embedded in
//! the program. `en.json` is generated from the code by a test and kept in step by it.

use std::collections::HashMap;
use std::sync::OnceLock;
#[cfg(not(test))]
use std::sync::RwLock;

use crate::commands::Cmd;

/// The languages that have a translation file, by code.
const FILES: &[(&str, &str)] = &[("en", include_str!("../../../assets/locales/en.json")), ("nb", include_str!("../../../assets/locales/nb.json"))];

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
        FILES
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

/// Whether language `code` has translated text.
pub fn available(code: &str) -> bool {
    tables().contains_key(code)
}

/// Switch the interface language. An unknown code leaves English.
pub fn set_language(code: &str) {
    let code = if available(code) { code } else { "en" };
    set_active(code);
}

/// The language in use.
pub fn language() -> String {
    let code = active();
    if code.is_empty() { "en".into() } else { code }
}

/// The text for `key` in the active language; `english` when there is none.
pub fn tr(key: &str, english: &str) -> String {
    let lang = language();
    if lang != "en"
        && let Some(text) = tables().get(lang.as_str()).and_then(|t| t.get(key))
    {
        return text.clone();
    }
    english.to_string()
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
        ("tangent_tool.rs", include_str!("tangent_tool.rs")),
        ("taper_warp.rs", include_str!("taper_warp.rs")),
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

    fn locale_path(code: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/locales").join(format!("{code}.json"))
    }

    #[test]
    fn english_file_matches_the_code() {
        let want = english();
        if std::env::var_os("DARKBURN_WRITE_LOCALES").is_some() {
            std::fs::write(locale_path("en"), serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
        }
        let have: BTreeMap<String, String> = serde_json::from_str(FILES[0].1).unwrap();
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
        let en: BTreeMap<String, String> = serde_json::from_str(FILES[0].1).unwrap();
        let nb: BTreeMap<String, String> = serde_json::from_str(FILES[1].1).unwrap();
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

    #[test]
    fn translated_text_is_used_and_english_is_the_fallback() {
        // The language is global; this test is the only one that changes it.
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
    fn every_language_in_the_menu_is_named_in_itself() {
        assert!(crate::commands::LANGUAGES.iter().any(|(c, n)| *c == "nb" && *n == "Norsk bokm\u{e5}l"));
        assert!(available("nb") && available("en"));
    }
}
