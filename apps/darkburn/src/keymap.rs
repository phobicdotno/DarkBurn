//! Keyboard shortcuts: the defaults come from the command table, the user's changes are kept in
//! the preferences as overrides (command id to key text, empty for "no key"). Conflicts are
//! detected when a key is assigned. A keymap travels as a `.dbhk` file.

use std::collections::BTreeMap;
use std::sync::RwLock;

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::{Deserialize, Serialize};

use crate::commands::{self, Cmd};
use crate::i18n::tr;

/// The user's changes to the default keys.
pub type Overrides = BTreeMap<String, String>;

pub const FILE_FORMAT: &str = "darkburn-hotkeys";
pub const FILE_EXTENSION: &str = "dbhk";

static CURRENT: RwLock<Overrides> = RwLock::new(Overrides::new());

/// Refresh the snapshot the menus and the ribbon tooltips read.
pub fn publish(o: &Overrides) {
    if let Ok(mut c) = CURRENT.write()
        && *c != *o
    {
        *c = o.clone();
    }
}

/// The key text now bound to command `c` (its default unless changed).
pub fn key_for(c: &Cmd) -> String {
    key_in(&CURRENT.read().map(|o| o.clone()).unwrap_or_default(), c.id, default_key(c))
}

/// Keys of commands whose registry entry (in another module) has none: the laser keys that
/// LightBurn has (Pause, Ctrl+Break) and the jog keys. egui cannot tell the numpad from the
/// digit row, so the jog keys are Ctrl+Alt+arrows.
const EXTRA_DEFAULTS: &[(&str, &str)] = &[
    ("job.pause", "Pause"),
    ("job.stop", "Ctrl+Pause"),
    ("laser.jog_left", "Ctrl+Alt+Left"),
    ("laser.jog_right", "Ctrl+Alt+Right"),
    ("laser.jog_up", "Ctrl+Alt+Up"),
    ("laser.jog_down", "Ctrl+Alt+Down"),
];

/// The default key of a command.
pub fn default_key(c: &Cmd) -> &'static str {
    if !c.key.is_empty() {
        return c.key;
    }
    EXTRA_DEFAULTS.iter().find(|(id, _)| *id == c.id).map_or("", |(_, k)| k)
}

/// The key text for `id` given overrides and the default.
pub fn key_in(o: &Overrides, id: &str, default: &str) -> String {
    o.get(id).cloned().unwrap_or_else(|| default.to_string())
}

/// Every command that can have a key: id, label, default key.
pub fn bindable() -> Vec<(&'static str, &'static str, &'static str)> {
    commands::registry().iter().filter(|c| c.args.is_empty()).map(|c| (c.id, c.label, default_key(c))).collect()
}

/// The effective key of each bindable command (empty when it has none).
pub fn effective(o: &Overrides) -> BTreeMap<&'static str, String> {
    bindable().into_iter().map(|(id, _, default)| (id, key_in(o, id, default))).collect()
}

/// Shortcut table for the key handler: commands with a key, those with more modifiers first so
/// Ctrl+Shift+Z is not taken for Ctrl+Z.
pub fn table(o: &Overrides) -> Vec<(KeyboardShortcut, &'static str)> {
    let mut v: Vec<_> = effective(o).into_iter().filter_map(|(id, key)| commands::parse_key(&key).map(|k| (k, id))).collect();
    let weight = |k: &KeyboardShortcut| k.modifiers.command as u8 + k.modifiers.shift as u8 + k.modifiers.alt as u8;
    v.sort_by_key(|(k, id)| (std::cmp::Reverse(weight(k)), *id));
    v
}

/// Keys bound to more than one command: (key text, the commands).
pub fn conflicts(o: &Overrides) -> Vec<(String, Vec<&'static str>)> {
    let mut by_key: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for (id, key) in effective(o) {
        if let Some(k) = commands::parse_key(&key) {
            by_key.entry(format!("{:?}", (k.modifiers, k.logical_key))).or_default().push(id);
        }
    }
    by_key.into_values().filter(|ids| ids.len() > 1).map(|ids| (effective(o)[ids[0]].clone(), ids)).collect()
}

/// Why an assignment was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    UnknownCommand,
    BadKey,
    /// Another command (id) already has this key.
    Taken(&'static str),
}

/// The command that already uses `key`, ignoring `except`.
pub fn owner_of(o: &Overrides, key: &str, except: &str) -> Option<&'static str> {
    let want = commands::parse_key(key)?;
    effective(o).into_iter().find(|(id, k)| *id != except && commands::parse_key(k) == Some(want)).map(|(id, _)| id)
}

/// Give command `id` the key `key` ("" clears it). Without `take`, a key another command uses is
/// refused; with it, the other command loses the key.
pub fn assign(o: &mut Overrides, id: &str, key: &str, take: bool) -> Result<(), Refused> {
    let Some((_, _, default)) = bindable().into_iter().find(|(i, _, _)| *i == id) else {
        return Err(Refused::UnknownCommand);
    };
    if !key.is_empty() && commands::parse_key(key).is_none() {
        return Err(Refused::BadKey);
    }
    if !key.is_empty()
        && let Some(other) = owner_of(o, key, id)
    {
        if !take {
            return Err(Refused::Taken(other));
        }
        o.insert(other.to_string(), String::new());
    }
    if key == default {
        o.remove(id);
    } else {
        o.insert(id.to_string(), key.to_string());
    }
    Ok(())
}

/// Back to the default key of `id`.
pub fn reset(o: &mut Overrides, id: &str) {
    o.remove(id);
}

/// The text of a key press, as the command table writes it (`Ctrl+Shift+Z`, `Del`, `PgUp`).
pub fn key_text(m: Modifiers, key: Key) -> String {
    let mut s = String::new();
    if m.command {
        s += "Ctrl+";
    }
    if m.alt {
        s += "Alt+";
    }
    if m.shift {
        s += "Shift+";
    }
    s += match key {
        Key::Delete => "Del",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDn",
        Key::Period => ".",
        Key::Comma => ",",
        Key::Equals => "=",
        Key::Minus => "-",
        Key::Plus => "+",
        Key::Backtick => "`",
        other => other.name(),
    };
    s
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    keys: BTreeMap<String, String>,
}

/// A `.dbhk` file holding the effective key of every command.
pub fn to_file(o: &Overrides) -> String {
    let keys = effective(o).into_iter().map(|(id, k)| (id.to_string(), k)).collect();
    serde_json::to_string_pretty(&File { format: FILE_FORMAT.into(), version: 1, keys }).unwrap_or_default()
}

/// The overrides a `.dbhk` file describes (only where it differs from the defaults).
pub fn from_file(text: &str) -> Result<Overrides, String> {
    let f: File = serde_json::from_str(text).map_err(|e| format!("{} ({e})", tr("hotkeys.bad_file", "This is not a hotkey file")))?;
    if f.format != FILE_FORMAT {
        return Err(tr("hotkeys.bad_file", "This is not a hotkey file"));
    }
    let mut o = Overrides::new();
    for (id, key) in f.keys {
        // Commands this version does not know are skipped, so files travel between versions.
        if bindable().iter().any(|(i, _, _)| *i == id) {
            if !key.is_empty() && commands::parse_key(&key).is_none() {
                return Err(format!("{}: {key}", tr("hotkeys.bad_key", "Not a key")));
            }
            let default = bindable().into_iter().find(|(i, _, _)| *i == id).map(|(_, _, d)| d).unwrap_or("");
            if key != default {
                o.insert(id, key);
            }
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_conflicts_and_all_parse() {
        for (id, _, key) in bindable() {
            if !key.is_empty() {
                assert!(commands::parse_key(key).is_some(), "{id}: {key}");
            }
        }
        assert!(conflicts(&Overrides::new()).is_empty(), "{:?}", conflicts(&Overrides::new()));
    }

    #[test]
    fn key_text_round_trips_through_the_parser() {
        for (id, _, key) in bindable() {
            // Alt+* is Shift+8 as far as egui can tell.
            if let Some(k) = commands::parse_key(key).filter(|_| !key.ends_with('*')) {
                assert_eq!(key_text(k.modifiers, k.logical_key), key, "{id}");
            }
        }
    }

    #[test]
    fn remapping_applies_and_default_is_kept_clean() {
        let mut o = Overrides::new();
        assign(&mut o, "edit.undo", "Ctrl+Alt+U", false).unwrap();
        assert_eq!(key_in(&o, "edit.undo", "Ctrl+Z"), "Ctrl+Alt+U");
        let t = table(&o);
        assert!(t.iter().any(|(k, id)| *id == "edit.undo" && commands::parse_key("Ctrl+Alt+U") == Some(*k)));
        assert!(!t.iter().any(|(k, id)| *id == "edit.undo" && commands::parse_key("Ctrl+Z") == Some(*k)));
        assign(&mut o, "edit.undo", "Ctrl+Z", false).unwrap();
        assert!(o.is_empty(), "going back to the default drops the override");
    }

    #[test]
    fn conflicts_are_detected_and_can_be_taken() {
        let mut o = Overrides::new();
        assert_eq!(assign(&mut o, "edit.redo", "Ctrl+Z", false), Err(Refused::Taken("edit.undo")));
        assert!(o.is_empty());
        assert_eq!(assign(&mut o, "edit.redo", "Ctrl+Z", true), Ok(()));
        assert_eq!(effective(&o)["edit.undo"], "");
        assert_eq!(effective(&o)["edit.redo"], "Ctrl+Z");
        assert!(conflicts(&o).is_empty());
        // A conflict made by hand (an imported file) is reported.
        let mut bad = Overrides::new();
        bad.insert("edit.redo".into(), "Ctrl+Z".into());
        let c = conflicts(&bad);
        assert_eq!(c.len(), 1);
        assert!(c[0].1.contains(&"edit.undo") && c[0].1.contains(&"edit.redo"));
        assert_eq!(assign(&mut o, "no.such", "F5", false), Err(Refused::UnknownCommand));
        assert_eq!(assign(&mut o, "edit.undo", "Ctrl+Banana", false), Err(Refused::BadKey));
    }

    #[test]
    fn hotkey_files_round_trip() {
        let mut o = Overrides::new();
        assign(&mut o, "file.save", "F2", false).unwrap();
        assign(&mut o, "edit.select_all", "", false).unwrap();
        let text = to_file(&o);
        assert!(text.contains(FILE_FORMAT));
        assert_eq!(from_file(&text).unwrap(), o);
        assert!(from_file("{}").is_err());
        assert!(from_file(r#"{"format":"darkburn-hotkeys","version":1,"keys":{"file.save":"Ctrl+Banana"}}"#).is_err());
        let skip = from_file(r#"{"format":"darkburn-hotkeys","version":1,"keys":{"future.command":"F9"}}"#).unwrap();
        assert!(skip.is_empty());
    }
}
